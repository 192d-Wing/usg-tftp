# Phase 4: Worker Thread Pool Architecture

**Date**: 2026-01-19
**Status**: In Implementation
**Goal**: Implement NGINX-style multi-threaded worker pool for improved CPU utilization and concurrent throughput

---

## 📋 Executive Summary

Phase 4 introduces a **master-worker thread pool architecture** to improve Snow-Owl TFTP's performance under high concurrent load. By distributing client processing across multiple worker threads, we can:

- **Utilize multiple CPU cores** (currently limited to single-threaded Tokio runtime)
- **Achieve 2-4x concurrent client capacity**
- **Reduce latency under load** through better load distribution
- **Maintain backward compatibility** with existing configurations

---

## 🎯 Design Goals

### Primary Objectives

1. **Multi-core CPU utilization**: Use 4-8 worker threads to parallelize packet processing
2. **Better load distribution**: Round-robin packet distribution prevents hot-spotting
3. **Improved batching**: Master thread focuses on efficient packet batching
4. **Scalability**: Linear performance scaling with worker count (up to core count)

### Performance Targets

| Metric | Current | Target | Improvement |
|--------|---------|--------|-------------|
| **Concurrent clients** | ~100 | 200-400 | 2-4x |
| **CPU utilization** | 1 core | 4-8 cores | 4-8x |
| **Throughput under load** | Baseline | +100-200% | 2-3x |
| **P99 latency** | Baseline | -30-50% | Better |

---

## 🏗️ Architecture Design

### Current Architecture (Phase 3)

```
┌──────────────────────────────────────┐
│  Single Event Loop (Tokio Runtime)   │
│  ┌────────────────────────────────┐  │
│  │  recvmmsg() batch receive      │  │
│  └───────────┬────────────────────┘  │
│              │                        │
│              ├─► tokio::spawn Client 1│
│              ├─► tokio::spawn Client 2│
│              ├─► tokio::spawn Client 3│
│              └─► tokio::spawn Client N│
│                                        │
│  All tasks run on single thread       │
└──────────────────────────────────────┘
```

**Limitations:**
- Single CPU core utilization
- Tokio cooperative scheduling can cause head-of-line blocking
- Limited by single-threaded event loop performance

### Proposed Architecture (Phase 4)

```
┌─────────────────────────────────────────────────────────────┐
│                    Master Thread                             │
│  ┌────────────────────────────────────────────────────┐     │
│  │  recvmmsg() - Batch receive up to 32 packets      │     │
│  │  • Dedicated to network I/O                        │     │
│  │  • Timeout-based accumulation (1ms)                │     │
│  │  • No client processing overhead                   │     │
│  └───────────────┬────────────────────────────────────┘     │
└──────────────────┼──────────────────────────────────────────┘
                   │
         Round-robin distribution
                   │
    ┌──────────────┼──────────────┬────────────┐
    │              │              │            │
    ▼              ▼              ▼            ▼
┌────────┐    ┌────────┐    ┌────────┐   ┌────────┐
│Worker 1│    │Worker 2│    │Worker 3│...│Worker N│
│        │    │        │    │        │   │        │
│Client A│    │Client D│    │Client G│   │Client J│
│Client B│    │Client E│    │Client H│   │Client K│
│Client C│    │Client F│    │Client I│   │Client L│
└───┬────┘    └───┬────┘    └───┬────┘   └───┬────┘
    │             │             │            │
    └──────┬──────┴─────┬───────┴────────────┘
           │            │
           ▼            ▼
    ┌─────────────────────────────────────┐
    │        Sender Thread                 │
    │  ┌──────────────────────────────┐   │
    │  │  Batch responses             │   │
    │  │  sendmmsg() up to 32 packets │   │
    │  │  • Low latency sending       │   │
    │  └──────────────────────────────┘   │
    └─────────────────────────────────────┘
```

**Benefits:**
- Master thread dedicated to fast packet reception
- Worker threads parallelize client processing
- Sender thread batches responses efficiently
- True multi-core utilization

---

## 🔧 Implementation Components

### 1. Data Structures

```rust
/// Incoming packet from master to worker
pub struct IncomingPacket {
    /// Raw packet data
    data: Vec<u8>,
    /// Client socket address
    addr: SocketAddr,
    /// Reception timestamp for metrics
    timestamp: Instant,
}

/// Outgoing response from worker to sender
pub struct OutgoingPacket {
    /// Response packet data
    data: Vec<u8>,
    /// Destination address
    addr: SocketAddr,
    /// Original timestamp for latency tracking
    timestamp: Instant,
}

/// Worker thread statistics
pub struct WorkerStats {
    worker_id: usize,
    packets_processed: AtomicU64,
    total_processing_time_us: AtomicU64,
    errors: AtomicU64,
}
```

### 2. Configuration

```rust
/// Worker thread pool configuration (Phase 4)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkerPoolConfig {
    /// Enable worker thread pool (Phase 4)
    /// When disabled, uses Phase 3 single-threaded architecture
    pub enabled: bool,

    /// Number of worker threads
    /// Recommended: number of CPU cores - 2 (reserve for master/sender)
    /// Valid range: 1-32
    pub worker_count: usize,

    /// Channel buffer size between master and workers
    /// Higher values improve throughput but increase latency
    /// Default: 256 packets per worker
    pub worker_channel_size: usize,

    /// Channel buffer size between workers and sender
    /// Default: 512 packets (shared by all workers)
    pub sender_channel_size: usize,

    /// Load balancing strategy
    pub load_balance_strategy: LoadBalanceStrategy,

    /// Enable worker thread affinity (pin to CPU cores)
    /// Improves cache locality but reduces flexibility
    /// Default: false
    pub enable_cpu_affinity: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadBalanceStrategy {
    /// Simple round-robin distribution
    RoundRobin,
    /// Hash client address to worker (session affinity)
    /// Better cache locality for repeated requests
    ClientHash,
    /// Send to worker with smallest queue (requires atomic checks)
    LeastLoaded,
}

impl Default for WorkerPoolConfig {
    fn default() -> Self {
        // Auto-detect CPU count, reserve 2 for master/sender
        let cpu_count = num_cpus::get();
        let worker_count = (cpu_count.saturating_sub(2)).max(1).min(8);

        Self {
            enabled: false, // Opt-in for Phase 4
            worker_count,
            worker_channel_size: 256,
            sender_channel_size: 512,
            load_balance_strategy: LoadBalanceStrategy::RoundRobin,
            enable_cpu_affinity: false,
        }
    }
}
```

### 3. Master Thread (Receiver)

```rust
/// Master thread: Dedicated to receiving packets and distributing to workers
async fn master_receiver_loop(
    socket: Arc<UdpSocket>,
    workers: Vec<mpsc::Sender<IncomingPacket>>,
    config: Arc<TftpConfig>,
    stats: Arc<MasterStats>,
) -> Result<()> {
    let batch_size = config.performance.platform.batch.max_batch_size;
    let batch_timeout_us = config.performance.platform.batch.batch_timeout_us;
    let strategy = config.performance.platform.worker_pool.load_balance_strategy;

    let mut worker_index: usize = 0;
    let worker_count = workers.len();

    info!("Master receiver starting with {} workers", worker_count);

    loop {
        // Batch receive packets using recvmmsg()
        let mut buffers: Vec<Vec<u8>> = (0..batch_size)
            .map(|_| vec![0u8; MAX_PACKET_SIZE])
            .collect();

        match batch_recv_packets(&socket, &mut buffers, batch_size, batch_timeout_us) {
            Ok(packets) if !packets.is_empty() => {
                let timestamp = Instant::now();
                stats.packets_received.fetch_add(packets.len() as u64, Ordering::Relaxed);

                // Distribute packets to workers
                for (i, (size, client_addr)) in packets.iter().enumerate() {
                    let mut data = buffers[i][..*size].to_vec();

                    let packet = IncomingPacket {
                        data,
                        addr: *client_addr,
                        timestamp,
                    };

                    // Select worker based on strategy
                    let worker_idx = match strategy {
                        LoadBalanceStrategy::RoundRobin => {
                            let idx = worker_index % worker_count;
                            worker_index = worker_index.wrapping_add(1);
                            idx
                        }
                        LoadBalanceStrategy::ClientHash => {
                            // Hash client IP:port for session affinity
                            use std::collections::hash_map::DefaultHasher;
                            use std::hash::{Hash, Hasher};
                            let mut hasher = DefaultHasher::new();
                            client_addr.hash(&mut hasher);
                            hasher.finish() as usize % worker_count
                        }
                        LoadBalanceStrategy::LeastLoaded => {
                            // Find worker with shortest queue (simple heuristic)
                            // In real implementation, track queue lengths
                            worker_index % worker_count
                        }
                    };

                    // Send to worker (non-blocking)
                    if let Err(e) = workers[worker_idx].try_send(packet) {
                        warn!("Worker {} channel full, dropping packet: {}", worker_idx, e);
                        stats.packets_dropped.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            Ok(_) => {
                // Timeout with no packets, retry
                continue;
            }
            Err(e) => {
                error!("Master receiver error: {}", e);
                stats.errors.fetch_add(1, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
}
```

### 4. Worker Threads

```rust
/// Worker thread: Process TFTP client requests
async fn worker_thread(
    worker_id: usize,
    mut rx: mpsc::Receiver<IncomingPacket>,
    tx: mpsc::Sender<OutgoingPacket>,
    config: Arc<TftpConfig>,
    stats: Arc<WorkerStats>,
) -> Result<()> {
    info!("Worker {} started", worker_id);

    while let Some(packet) = rx.recv().await {
        let start = Instant::now();

        // Process TFTP packet (handle_client logic)
        match process_tftp_packet(packet, &config).await {
            Ok(responses) => {
                // Send responses to sender thread
                for response in responses {
                    if let Err(e) = tx.send(response).await {
                        error!("Worker {}: Failed to send response: {}", worker_id, e);
                        stats.errors.fetch_add(1, Ordering::Relaxed);
                    }
                }

                stats.packets_processed.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => {
                error!("Worker {}: Error processing packet: {}", worker_id, e);
                stats.errors.fetch_add(1, Ordering::Relaxed);
            }
        }

        let elapsed = start.elapsed().as_micros() as u64;
        stats.total_processing_time_us.fetch_add(elapsed, Ordering::Relaxed);
    }

    info!("Worker {} shutting down", worker_id);
    Ok(())
}

/// Process a single TFTP packet and generate responses
async fn process_tftp_packet(
    packet: IncomingPacket,
    config: &TftpConfig,
) -> Result<Vec<OutgoingPacket>> {
    // Extract existing handle_client logic here
    // Parse opcode, handle RRQ/WRQ/DATA/ACK/ERROR
    // Return vector of response packets

    // This will be a refactored version of the existing handle_client
    // function that returns packets instead of sending directly
    todo!("Implement packet processing")
}
```

### 5. Sender Thread

```rust
/// Sender thread: Batch outgoing responses using sendmmsg()
async fn sender_thread(
    mut rx: mpsc::Receiver<OutgoingPacket>,
    socket: Arc<UdpSocket>,
    config: Arc<TftpConfig>,
    stats: Arc<SenderStats>,
) -> Result<()> {
    let batch_size = config.performance.platform.batch.max_batch_size;
    let batch_timeout = Duration::from_micros(config.performance.platform.batch.batch_timeout_us);

    let mut batch = Vec::with_capacity(batch_size);

    info!("Sender thread started");

    loop {
        // Collect responses for batching
        tokio::select! {
            // Try to fill batch quickly
            opt = rx.recv() => {
                if let Some(packet) = opt {
                    batch.push(packet);

                    // Fill rest of batch non-blocking
                    while batch.len() < batch_size {
                        match rx.try_recv() {
                            Ok(pkt) => batch.push(pkt),
                            Err(_) => break,
                        }
                    }
                } else {
                    // Channel closed, shutdown
                    break;
                }
            }
            // Timeout to prevent stale batches
            _ = tokio::time::sleep(batch_timeout), if !batch.is_empty() => {
                // Timeout expired, send what we have
            }
        }

        // Send batch using sendmmsg()
        if !batch.is_empty() {
            match batch_send_packets(&socket, &batch).await {
                Ok(sent) => {
                    stats.packets_sent.fetch_add(sent as u64, Ordering::Relaxed);
                    stats.batches_sent.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    error!("Sender thread error: {}", e);
                    stats.errors.fetch_add(1, Ordering::Relaxed);
                }
            }
            batch.clear();
        }
    }

    info!("Sender thread shutting down");
    Ok(())
}
```

---

## 📊 Performance Expectations

### Throughput Improvements

| Scenario | Phase 3 | Phase 4 | Improvement |
|----------|---------|---------|-------------|
| **10 concurrent clients** | 100 MB/s | 105 MB/s | +5% (overhead) |
| **50 concurrent clients** | 400 MB/s | 800 MB/s | **+100%** |
| **100 concurrent clients** | 600 MB/s | 1400 MB/s | **+133%** |
| **200 concurrent clients** | 700 MB/s | 1800 MB/s | **+157%** |

### CPU Utilization

| Phase | Cores Used | Efficiency |
|-------|-----------|-----------|
| Phase 3 | 1 core @ 100% | Limited |
| Phase 4 (4 workers) | 6 cores @ 80% | 4.8x effective |
| Phase 4 (8 workers) | 10 cores @ 75% | 7.5x effective |

### Latency Under Load

| Load Level | Phase 3 P99 | Phase 4 P99 | Improvement |
|-----------|-------------|-------------|-------------|
| Light (10 clients) | 5ms | 5ms | Same |
| Medium (50 clients) | 20ms | 12ms | **-40%** |
| Heavy (100 clients) | 50ms | 25ms | **-50%** |

---

## 🚧 Implementation Phases

### Phase 4.1: Core Infrastructure (Week 1)

- [ ] Add `WorkerPoolConfig` to config.rs
- [ ] Create `worker_pool.rs` module with data structures
- [ ] Implement master receiver loop
- [ ] Implement worker threads
- [ ] Implement sender thread
- [ ] Add unit tests for components

### Phase 4.2: Integration (Week 2)

- [ ] Refactor `handle_client` into `process_tftp_packet`
- [ ] Integrate worker pool into main.rs
- [ ] Add configuration flag to enable/disable
- [ ] Implement graceful shutdown
- [ ] Add comprehensive logging

### Phase 4.3: Optimization (Week 3)

- [ ] Implement load balancing strategies
- [ ] Add CPU affinity support
- [ ] Optimize channel sizes
- [ ] Add worker statistics and metrics
- [ ] Performance profiling

### Phase 4.4: Testing & Documentation (Week 4)

- [ ] Create benchmark suite
- [ ] Test with 10, 50, 100, 200 concurrent clients
- [ ] Measure CPU utilization
- [ ] Document configuration tuning
- [ ] Update README and guides

---

## ⚠️ Risks and Mitigations

### Risk 1: Increased Complexity

**Risk**: Multi-threaded architecture is harder to debug and maintain
**Mitigation**:
- Keep worker pool as optional feature
- Comprehensive logging and metrics
- Unit tests for each component
- Clear documentation

### Risk 2: Channel Overhead

**Risk**: Channel communication overhead might negate benefits at low loads
**Mitigation**:
- Only enable for high-concurrency scenarios (>20 clients)
- Use bounded channels to prevent memory bloat
- Benchmark thoroughly at various loads

### Risk 3: State Management

**Risk**: Client session state harder to manage across workers
**Mitigation**:
- Use client-hash strategy for session affinity
- Keep worker pool stateless (stateful sessions handled in Tokio tasks)
- Document multi-threaded considerations

### Risk 4: Platform Compatibility

**Risk**: Worker pool may not benefit all platforms equally
**Mitigation**:
- Make it opt-in with feature flag
- Fallback to Phase 3 architecture if disabled
- Test on Linux, FreeBSD, macOS

---

## 🎯 Success Criteria

Phase 4 is considered successful if:

1. ✅ **2x throughput** at 100+ concurrent clients
2. ✅ **Multi-core utilization** (4-8 cores active)
3. ✅ **50% latency reduction** under load (P99)
4. ✅ **Zero regressions** at low concurrency (<10 clients)
5. ✅ **Backward compatible** with existing configs
6. ✅ **Comprehensive documentation** and benchmarks

---

## 📚 References

- [NGINX Architecture](https://www.nginx.com/blog/inside-nginx-how-we-designed-for-performance-scale/)
- [HAProxy Multi-threading](https://www.haproxy.com/blog/multithreading-in-haproxy/)
- [Tokio Multi-threaded Runtime](https://docs.rs/tokio/latest/tokio/runtime/)
- [Performance Optimization Plan](PERFORMANCE_OPTIMIZATION_PLAN.md)
- [RFC 7440 Implementation](RFC7440_IMPLEMENTATION_SUMMARY.md)

---

**Next Steps**: Proceed with Phase 4.1 implementation - add configuration and create worker_pool module.
