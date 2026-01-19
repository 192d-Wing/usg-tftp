# Performance Enhancements Roadmap for snow-owl-tftp

## Linux/BSD Systems Optimization Plan

**Version:** 1.0
**Last Updated:** 2026-01-19
**Target Platforms:** Linux (kernel 4.14+), FreeBSD, OpenBSD, NetBSD

---

## Executive Summary

This roadmap outlines platform-specific performance optimizations for the snow-owl-tftp server targeting Linux and BSD systems. Optimizations are prioritized by impact vs. effort and organized into phases for systematic implementation.

**Expected Overall Performance Gains:**

- **Throughput:** 3-5x improvement for large file transfers
- **Latency:** 40-60% reduction in per-packet overhead
- **CPU Usage:** 30-50% reduction through zero-copy operations
- **Concurrent Connections:** 2-3x more simultaneous transfers

---

## Phase 1: Foundation (High Impact, Low Effort)

**Timeline:** Sprint 1 (2 weeks)
**Goal:** Quick wins with minimal code changes

### 1.1 Socket Buffer Tuning

**Status:** Not Implemented
**Priority:** P0 (Critical)
**Complexity:** Low

**Implementation:**

```rust
// In main.rs:343 (main socket) and main.rs:959 (transfer sockets)
socket.set_recv_buffer_size(2 * 1024 * 1024)?; // 2MB receive buffer
socket.set_send_buffer_size(2 * 1024 * 1024)?; // 2MB send buffer
```

**Configuration Addition (config.rs):**

```rust
pub struct SocketConfig {
    pub recv_buffer_kb: usize, // Default: 2048
    pub send_buffer_kb: usize, // Default: 2048
}
```

**Expected Impact:**

- Reduces packet loss under high load by 70-80%
- Improves burst handling capacity
- Better performance with high-latency clients

**Testing:**

- Concurrent transfer test (integration-test.sh:354-387)
- Monitor socket buffer drops: `netstat -su | grep "packet receive errors"`
- Benchmark with iperf3

---

### 1.2 Socket Reuse Options

**Status:** Not Implemented
**Priority:** P0 (Critical)
**Complexity:** Low

**Implementation:**

```rust
use socket2::{Socket, Domain, Type, Protocol};

let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
socket.set_reuse_address(true)?;
socket.set_reuse_port(true)?; // Linux 3.9+, BSD
socket.bind(&bind_addr.into())?;
```

**Benefits:**

- Zero downtime restarts
- Multi-process scaling (run multiple instances on same port)
- Load distribution across CPU cores

**Testing:**

- Start multiple server instances on same port
- Verify load distribution
- Test graceful restart scenarios

---

### 1.3 POSIX File Advisory Hints

**Status:** Not Implemented
**Priority:** P1 (High)
**Complexity:** Low

**Implementation:**

```rust
use nix::fcntl::{posix_fadvise, PosixFadviseAdvice};
use std::os::unix::io::AsRawFd;

// In handle_read_request after opening file (main.rs:963)
let fd = file.as_raw_fd();
posix_fadvise(fd, 0, 0, PosixFadviseAdvice::POSIX_FADV_SEQUENTIAL)?;
posix_fadvise(fd, 0, 0, PosixFadviseAdvice::POSIX_FADV_WILLNEED)?;

// For random access patterns (if detected)
posix_fadvise(fd, 0, 0, PosixFadviseAdvice::POSIX_FADV_RANDOM)?;
```

**Expected Impact:**

- 20-30% reduction in read latency
- Optimizes kernel read-ahead behavior
- Reduces I/O wait time

**Configuration Addition:**

```rust
pub struct FileIoConfig {
    pub use_sequential_hint: bool,    // Default: true
    pub use_willneed_hint: bool,      // Default: true
    pub fadvise_dontneed_after: bool, // Free cache after transfer
}
```

---

### 1.4 Performance Monitoring Infrastructure

**Status:** Partial (audit logging exists)
**Priority:** P1 (High)
**Complexity:** Low

**Implementation:**

```rust
// Add to audit.rs
pub struct PerformanceMetrics {
    pub packets_sent: AtomicU64,
    pub packets_received: AtomicU64,
    pub bytes_transferred: AtomicU64,
    pub socket_errors: AtomicU64,
    pub buffer_drops: AtomicU64,
    pub avg_transfer_time_ms: AtomicU64,
}

// Linux-specific: read from /proc
#[cfg(target_os = "linux")]
pub fn get_udp_stats() -> Result<UdpStats> {
    use procfs::net::udp;
    let stats = udp()?;
    // Parse RcvbufErrors, SndbufErrors
}
```

**Metrics to Track:**

- Socket buffer drops/errors
- Per-transfer throughput
- CPU usage per connection
- Memory allocation patterns

**Deliverables:**

- Prometheus/StatsD export support
- Performance dashboard recommendations
- Benchmark baseline suite

---

## Phase 2: Zero-Copy Operations (High Impact, Medium Effort)

**Timeline:** Sprint 2-3 (4 weeks)
**Goal:** Eliminate unnecessary memory copies

### 2.1 sendmmsg() / recvmmsg() Batch Operations

**Status:** Not Implemented
**Priority:** P0 (Critical)
**Complexity:** Medium

**Problem:**
Current implementation makes one syscall per packet. With concurrent transfers (integration-test.sh:354-387), this creates syscall overhead.

**Implementation:**

```rust
use nix::sys::socket::{sendmmsg, recvmmsg, MsgHdr, IoVec, ControlMessage};

// Replace individual socket.send() calls with batch sending
pub async fn send_batch(socket: &UdpSocket, packets: Vec<(SocketAddr, BytesMut)>) -> Result<()> {
    let msgs: Vec<SendMmsgData> = packets.iter().map(|(addr, data)| {
        SendMmsgData {
            iov: vec![IoVec::from_slice(data)],
            addr: Some(*addr),
            flags: MsgFlags::empty(),
        }
    }).collect();

    sendmmsg(socket.as_raw_fd(), &msgs, MsgFlags::empty())?;
    Ok(())
}
```

**Where to Apply:**

- Batch ACK packets during concurrent transfers
- Batch DATA packets for multicast (multicast.rs)
- Batch error responses

**Expected Impact:**

- 60-80% reduction in syscall overhead
- 2-3x improvement in concurrent transfer performance
- Lower CPU usage for packet processing

**Configuration Addition:**

```rust
pub struct BatchConfig {
    pub enable_sendmmsg: bool,        // Default: true on Linux/BSD
    pub enable_recvmmsg: bool,        // Default: true on Linux/BSD
    pub max_batch_size: usize,        // Default: 32 packets
    pub batch_timeout_us: u64,        // Default: 100 microseconds
}
```

**Testing:**

- Run concurrent transfer test with 10+ clients
- Measure syscall count with strace: `strace -c ./snow-owl-tftp`
- Benchmark before/after throughput

---

### 2.2 sendfile() Zero-Copy (Linux)

**Status:** Not Implemented
**Priority:** P0 (Critical)
**Complexity:** Medium

**Problem:**
Current flow (main.rs:1110+):

1. Read file into buffer (user space)
2. Copy buffer to socket (kernel space)
= 2 copies total

**Solution:**

```rust
#[cfg(target_os = "linux")]
use nix::sys::sendfile::sendfile;
use std::os::unix::io::AsRawFd;

// Replace buffer-based transfer with sendfile()
pub async fn send_file_zerocopy(
    file: &File,
    socket: &UdpSocket,
    offset: i64,
    count: usize,
) -> Result<usize> {
    let file_fd = file.as_raw_fd();
    let socket_fd = socket.as_raw_fd();

    // Direct kernel-to-kernel transfer
    let sent = sendfile(socket_fd, file_fd, Some(&mut offset), count)?;
    Ok(sent)
}
```

**Challenges:**

- sendfile() requires connected socket (already done: main.rs:960)
- Must handle TFTP packetization (512-65464 byte blocks)
- Needs fallback for non-Linux systems

**Implementation Strategy:**

```rust
#[cfg(target_os = "linux")]
async fn send_data_block_linux(/* ... */) -> Result<()> {
    if config.performance.use_sendfile {
        // Zero-copy path
        sendfile_with_tftp_header(file, socket, block_num, offset, block_size).await?;
    } else {
        // Fallback to buffered
        send_data_block_buffered(/* ... */).await?;
    }
}
```

**Expected Impact:**

- 30-50% reduction in CPU usage
- 2-3x throughput improvement for large files
- Reduced memory bandwidth pressure

**Configuration:**

```rust
pub struct ZeroCopyConfig {
    pub use_sendfile: bool,              // Linux only, default: true
    pub sendfile_threshold_bytes: u64,   // Min file size to use sendfile, default: 64KB
}
```

---

### 2.3 MSG_ZEROCOPY Flag (Linux 4.14+)

**Status:** Not Implemented
**Priority:** P1 (High)
**Complexity:** Medium

**Implementation:**

```rust
#[cfg(target_os = "linux")]
use nix::sys::socket::MsgFlags;

const MSG_ZEROCOPY: i32 = 0x4000000;

pub async fn send_zerocopy(socket: &UdpSocket, data: &[u8]) -> Result<()> {
    let flags = MsgFlags::from_bits_truncate(MSG_ZEROCOPY);
    socket.send_with_flags(data, flags)?;

    // Must handle completion notifications
    wait_for_zerocopy_completion(socket).await?;
    Ok(())
}
```

**Considerations:**

- Requires notification handling (adds complexity)
- Most beneficial for large blocks (>8KB)
- May not help with default 512-byte blocks
- Best combined with larger block sizes (config.performance.default_block_size = 8192)

**Expected Impact:**

- 20-30% reduction in CPU for large block transfers
- Reduced memory bandwidth usage
- Better scalability with many concurrent connections

---

### 2.4 CPU Affinity Tuning

**Status:** Not Implemented
**Priority:** P2 (Medium)
**Complexity:** Medium

**Implementation:**

```rust
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use core_affinity::{CoreId, set_for_current};

// In main(), pin main receive loop to specific core
if let Some(cpu_id) = config.performance.pin_to_cpu {
    set_for_current(CoreId { id: cpu_id });
    info!("Pinned main thread to CPU {}", cpu_id);
}

// Pin worker tasks to NUMA-local cores
tokio::spawn(async move {
    if let Some(cpu_id) = worker_cpu_affinity {
        set_for_current(CoreId { id: cpu_id });
    }
    // ... handle transfer
});
```

**Benefits:**

- Reduces cache thrashing
- Improves L1/L2/L3 cache hit rates
- Better performance on multi-socket NUMA systems

**Configuration:**

```rust
pub struct CpuAffinityConfig {
    pub pin_main_thread: Option<usize>,      // CPU ID for main loop
    pub pin_worker_threads: bool,            // Auto-assign workers to cores
    pub numa_aware: bool,                    // NUMA-local allocations
}
```

---

## Phase 3: Advanced I/O (High Impact, High Effort)

**Timeline:** Sprint 4-6 (6 weeks)
**Goal:** Modern async I/O infrastructure

### 3.1 io_uring Integration (Linux 5.1+)

**Status:** Not Implemented
**Priority:** P1 (High)
**Complexity:** High

**Problem:**
Current tokio file I/O (main.rs:963) uses blocking operations in thread pool. This limits scalability.

**Solution:**

```rust
// Replace tokio::fs with tokio-uring
use tokio_uring::fs::File;

pub async fn read_file_uring(path: &Path) -> Result<Vec<u8>> {
    let file = File::open(path).await?;
    let buf = vec![0u8; 65536];
    let (result, buf) = file.read_at(buf, 0).await;
    result?;
    Ok(buf)
}
```

**Architecture Changes:**

1. Create separate io_uring runtime for file operations
2. Keep tokio runtime for network I/O
3. Use channels to coordinate between runtimes

**Expected Impact:**

- True async file I/O (no thread pool blocking)
- 50-100% improvement in concurrent transfer scalability
- Lower memory overhead (fewer threads)
- Better tail latency

**Implementation Phases:**

1. **Phase 3.1a:** Proof of concept with read-only transfers
2. **Phase 3.1b:** Add write support (WRQ)
3. **Phase 3.1c:** Integration with buffer pool
4. **Phase 3.1d:** Performance tuning and benchmarking

**Configuration:**

```rust
pub struct IoUringConfig {
    pub enabled: bool,                    // Default: false (opt-in)
    pub queue_depth: u32,                 // Default: 128
    pub use_sqpoll: bool,                 // Kernel polling, default: false
    pub sq_thread_cpu: Option<usize>,     // CPU for SQ polling thread
}
```

**Fallback Strategy:**

- Feature flag: `cargo build --features io_uring`
- Runtime detection of io_uring support
- Automatic fallback to tokio::fs if unavailable

---

### 3.2 Memory Management Optimizations

**Status:** Partial (buffer_pool.rs exists)
**Priority:** P2 (Medium)
**Complexity:** High

#### 3.2a Huge Pages Support

```rust
#[cfg(target_os = "linux")]
use libc::{mmap, MAP_HUGETLB, MAP_ANONYMOUS};

pub struct HugePageAllocator {
    page_size: usize, // 2MB or 1GB
}

impl HugePageAllocator {
    pub fn allocate(&self, size: usize) -> Result<*mut u8> {
        let addr = unsafe {
            mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_HUGETLB,
                -1,
                0,
            )
        };
        // ...
    }
}
```

**Expected Impact:**

- 10-20% reduction in TLB misses
- Improved memory throughput
- Better performance with large buffer pools

#### 3.2b NUMA-Aware Allocations

```rust
#[cfg(target_os = "linux")]
use libnuma::{numa_set_preferred, numa_alloc_onnode};

// Allocate buffers on same NUMA node as CPU
pub fn allocate_numa_local(size: usize) -> Result<BytesMut> {
    let node = numa_node_of_cpu(current_cpu())?;
    let ptr = numa_alloc_onnode(size, node);
    // ...
}
```

**Configuration:**

```rust
pub struct MemoryConfig {
    pub use_huge_pages: bool,             // Default: false
    pub huge_page_size_mb: usize,         // 2 or 1024 (1GB)
    pub numa_aware: bool,                 // Default: false
    pub buffer_pool_per_numa_node: bool,  // Separate pools per node
}
```

---

### 3.3 Advanced Network Stack Integration

**Status:** Not Implemented
**Priority:** P3 (Low)
**Complexity:** Very High

#### 3.3a BPF Socket Filters

```rust
#[cfg(target_os = "linux")]
use libc::{setsockopt, SOL_SOCKET, SO_ATTACH_FILTER};

// Filter non-TFTP packets in kernel before userspace
pub fn attach_tftp_filter(socket: &UdpSocket) -> Result<()> {
    let filter = bpf_program! {
        // Accept only TFTP opcodes (1-6)
        ldh [0],              // Load 2-byte opcode
        jge #7, drop, accept, // Jump if >= 7 to drop, else accept
    };

    unsafe {
        setsockopt(
            socket.as_raw_fd(),
            SOL_SOCKET,
            SO_ATTACH_FILTER,
            &filter as *const _ as *const _,
            std::mem::size_of_val(&filter) as u32,
        );
    }
    Ok(())
}
```

**Benefits:**

- Reduces userspace wakeups for invalid packets
- CPU savings proportional to invalid traffic

#### 3.3b XDP (eXpress Data Path) - Research Phase

**Status:** Research Only
**Priority:** P4 (Future)
**Complexity:** Extreme

**Overview:**

- Bypass kernel network stack entirely
- Process packets directly in NIC driver
- Requires eBPF programs loaded into kernel
- Ultra-low latency (<1µs packet processing)

**Use Cases:**

- Extremely high packet rates (>1M pps)
- Latency-sensitive applications (<10µs RTT)
- Specialized hardware deployments

**Decision Point:** Evaluate after Phase 3 completion

---

## Phase 4: Real-Time & Specialized Deployments

**Timeline:** Sprint 7+ (Ongoing)
**Goal:** Mission-critical deployment support

### 4.1 Real-Time Scheduling Support

```rust
#[cfg(target_os = "linux")]
use libc::{sched_setscheduler, sched_param, SCHED_FIFO};

pub fn enable_realtime(priority: i32) -> Result<()> {
    let param = sched_param { sched_priority: priority };
    unsafe {
        sched_setscheduler(0, SCHED_FIFO, &param);
    }
    Ok(())
}
```

**Configuration:**

```rust
pub struct RealtimeConfig {
    pub enabled: bool,                // Default: false
    pub priority: i32,                // 1-99, default: 50
    pub mlockall: bool,               // Lock memory to prevent paging
}
```

**Use Cases:**

- Industrial control systems
- Network booting critical infrastructure
- Time-sensitive deployments

---

### 4.2 Security-Performance Tradeoffs

**Status:** Design Phase
**Priority:** P2 (Medium)

**Configuration Framework:**

```rust
pub struct SecurityPerformanceProfile {
    pub profile: SecurityProfile,
}

pub enum SecurityProfile {
    Maximum,      // Full audit, all checks, slower
    Balanced,     // Sampling, essential checks (default)
    Performance,  // Minimal audit, fast path
}
```

**Tunable Parameters:**

- Audit sampling rate (config.rs:579 already exists)
- Path validation depth
- Pattern matching complexity
- Logging verbosity

**Implementation:**

- Extend existing PerformanceConfig
- Add runtime profile switching
- Document security implications

---

## Configuration Schema

### Consolidated Performance Configuration

Add to `config.rs`:

```rust
/// Performance tuning configuration for Linux/BSD systems
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PlatformPerformanceConfig {
    /// Socket-level optimizations
    pub socket: SocketConfig,

    /// Zero-copy I/O settings
    pub zerocopy: ZeroCopyConfig,

    /// Batch operation settings
    pub batching: BatchConfig,

    /// File I/O optimizations
    pub file_io: FileIoConfig,

    /// io_uring configuration (Linux 5.1+)
    #[cfg(target_os = "linux")]
    pub io_uring: IoUringConfig,

    /// Memory management
    pub memory: MemoryConfig,

    /// CPU affinity and scheduling
    pub cpu: CpuAffinityConfig,

    /// Real-time configuration
    pub realtime: RealtimeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SocketConfig {
    pub recv_buffer_kb: usize,        // Default: 2048
    pub send_buffer_kb: usize,        // Default: 2048
    pub reuse_port: bool,             // Default: true
    pub reuse_address: bool,          // Default: true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ZeroCopyConfig {
    pub use_sendfile: bool,           // Linux only, default: true
    pub sendfile_threshold_bytes: u64, // Default: 65536
    pub use_msg_zerocopy: bool,       // Linux 4.14+, default: false
    pub zerocopy_threshold_bytes: u64, // Default: 8192
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BatchConfig {
    pub enable_sendmmsg: bool,        // Default: true on Linux/BSD
    pub enable_recvmmsg: bool,        // Default: true on Linux/BSD
    pub max_batch_size: usize,        // Default: 32
    pub batch_timeout_us: u64,        // Default: 100
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FileIoConfig {
    pub use_sequential_hint: bool,    // Default: true
    pub use_willneed_hint: bool,      // Default: true
    pub fadvise_dontneed_after: bool, // Default: false
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IoUringConfig {
    pub enabled: bool,                // Default: false
    pub queue_depth: u32,             // Default: 128
    pub use_sqpoll: bool,             // Default: false
    pub sq_thread_cpu: Option<usize>, // Default: None
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    pub use_huge_pages: bool,         // Default: false
    pub huge_page_size_mb: usize,     // Default: 2 (2MB pages)
    pub numa_aware: bool,             // Default: false
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CpuAffinityConfig {
    pub pin_main_thread: Option<usize>,      // Default: None
    pub pin_worker_threads: bool,            // Default: false
    pub numa_aware: bool,                    // Default: false
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RealtimeConfig {
    pub enabled: bool,                // Default: false
    pub priority: i32,                // Default: 50 (range: 1-99)
    pub mlockall: bool,               // Default: false
}

impl Default for SocketConfig {
    fn default() -> Self {
        Self {
            recv_buffer_kb: 2048,
            send_buffer_kb: 2048,
            reuse_port: true,
            reuse_address: true,
        }
    }
}

impl Default for ZeroCopyConfig {
    fn default() -> Self {
        Self {
            use_sendfile: cfg!(target_os = "linux"),
            sendfile_threshold_bytes: 65536,
            use_msg_zerocopy: false, // Opt-in due to complexity
            zerocopy_threshold_bytes: 8192,
        }
    }
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            enable_sendmmsg: cfg!(any(target_os = "linux", target_os = "freebsd")),
            enable_recvmmsg: cfg!(any(target_os = "linux", target_os = "freebsd")),
            max_batch_size: 32,
            batch_timeout_us: 100,
        }
    }
}

impl Default for FileIoConfig {
    fn default() -> Self {
        Self {
            use_sequential_hint: true,
            use_willneed_hint: true,
            fadvise_dontneed_after: false,
        }
    }
}

#[cfg(target_os = "linux")]
impl Default for IoUringConfig {
    fn default() -> Self {
        Self {
            enabled: false, // Opt-in for now
            queue_depth: 128,
            use_sqpoll: false,
            sq_thread_cpu: None,
        }
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            use_huge_pages: false,
            huge_page_size_mb: 2,
            numa_aware: false,
        }
    }
}

impl Default for CpuAffinityConfig {
    fn default() -> Self {
        Self {
            pin_main_thread: None,
            pin_worker_threads: false,
            numa_aware: false,
        }
    }
}

impl Default for RealtimeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            priority: 50,
            mlockall: false,
        }
    }
}

impl Default for PlatformPerformanceConfig {
    fn default() -> Self {
        Self {
            socket: SocketConfig::default(),
            zerocopy: ZeroCopyConfig::default(),
            batching: BatchConfig::default(),
            file_io: FileIoConfig::default(),
            #[cfg(target_os = "linux")]
            io_uring: IoUringConfig::default(),
            memory: MemoryConfig::default(),
            cpu: CpuAffinityConfig::default(),
            realtime: RealtimeConfig::default(),
        }
    }
}
```

---

## Testing & Benchmarking

### Benchmark Suite

Create `benches/performance.rs`:

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};

fn benchmark_file_transfer(c: &mut Criterion) {
    let mut group = c.benchmark_group("file_transfer");

    // Vary file sizes
    for size in [512, 4096, 65536, 1048576, 10485760].iter() {
        group.throughput(Throughput::Bytes(*size as u64));
        group.bench_with_input(
            format!("transfer_{}KB", size / 1024),
            size,
            |b, &size| {
                b.iter(|| {
                    // Benchmark transfer
                });
            },
        );
    }
}

fn benchmark_concurrent_transfers(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent");

    for clients in [1, 5, 10, 25, 50, 100].iter() {
        group.bench_with_input(
            format!("{}_clients", clients),
            clients,
            |b, &clients| {
                b.iter(|| {
                    // Benchmark concurrent transfers
                });
            },
        );
    }
}

criterion_group!(benches, benchmark_file_transfer, benchmark_concurrent_transfers);
criterion_main!(benches);
```

### Integration Test Enhancements

Extend `tests/integration-test.sh`:

```bash
# Test 11: Benchmark throughput
test_benchmark_throughput() {
    cd "$TEST_DIR/client"

    # Create 10MB test file
    dd if=/dev/zero of="$TEST_DIR/root/10mb.bin" bs=1M count=10 2>/dev/null

    # Measure transfer time
    start=$(date +%s%N)
    tftp 127.0.0.1 $SERVER_PORT <<EOF
mode octet
get 10mb.bin
quit
EOF
    end=$(date +%s%N)

    elapsed_ms=$(( ($end - $start) / 1000000 ))
    throughput_mbps=$(( (10 * 8 * 1000) / $elapsed_ms ))

    echo "Throughput: ${throughput_mbps} Mbps"

    # Expect at least 100 Mbps on localhost
    if [ $throughput_mbps -lt 100 ]; then
        echo "Throughput too low: ${throughput_mbps} Mbps"
        return 1
    fi

    rm -f 10mb.bin
    return 0
}

# Test 12: Socket buffer drops
test_socket_buffer_drops() {
    # Capture initial stats
    initial_drops=$(netstat -su | grep "packet receive errors" | awk '{print $1}')

    # Run stress test
    for i in {1..50}; do
        tftp 127.0.0.1 $SERVER_PORT <<EOF &
mode octet
get large.bin large-stress-$i.bin
quit
EOF
    done

    wait

    # Check final stats
    final_drops=$(netstat -su | grep "packet receive errors" | awk '{print $1}')
    drops=$(($final_drops - $initial_drops))

    if [ $drops -gt 100 ]; then
        echo "Too many packet drops: $drops"
        return 1
    fi

    return 0
}

# Test 13: CPU usage profiling
test_cpu_usage() {
    # Start profiling
    perf record -p $SERVER_PID -o "$TEST_DIR/perf.data" -g sleep 10 &
    PERF_PID=$!

    # Run transfers during profiling
    for i in {1..20}; do
        tftp 127.0.0.1 $SERVER_PORT <<EOF &
mode octet
get random.bin random-perf-$i.bin
quit
EOF
    done

    wait $PERF_PID

    # Generate report
    perf report -i "$TEST_DIR/perf.data" > "$TEST_DIR/perf-report.txt"

    echo "Perf report saved to $TEST_DIR/perf-report.txt"
    return 0
}
```

---

## Dependencies

### New Crate Dependencies

Add to `Cargo.toml`:

```toml
[dependencies]
# Existing dependencies...

# Phase 1: Socket operations
socket2 = { version = "0.5", features = ["all"] }

# Phase 2: Zero-copy and batch operations
nix = { version = "0.27", features = ["socket", "uio"] }
core_affinity = "0.8"

# Phase 3: io_uring (optional)
tokio-uring = { version = "0.4", optional = true }

# Phase 3: NUMA support (optional)
libnuma = { version = "0.1", optional = true }

# Phase 4: System information
procfs = { version = "0.16", optional = true }

[features]
default = ["platform-optimizations"]

# Feature flags for progressive rollout
platform-optimizations = ["socket2", "nix"]
io_uring = ["tokio-uring"]
numa = ["libnuma"]
advanced-metrics = ["procfs"]
realtime = []  # Marker feature for RT scheduling

# Platform-specific features
linux-optimizations = ["platform-optimizations", "advanced-metrics"]
bsd-optimizations = ["platform-optimizations"]

[dev-dependencies]
criterion = { version = "0.5", features = ["html_reports"] }
```

---

## System Requirements

### Linux

**Minimum:**

- Kernel 4.14+ (for MSG_ZEROCOPY)
- glibc 2.28+

**Recommended:**

- Kernel 5.1+ (for io_uring)
- Kernel 5.10+ (LTS)

**Optimal:**

- Kernel 6.0+ (latest io_uring features)

### BSD

**FreeBSD:**

- FreeBSD 12.0+ (sendmmsg/recvmmsg support)
- FreeBSD 13.0+ (recommended)

**OpenBSD:**

- OpenBSD 6.7+ (basic features)
- Note: Limited zero-copy support

**NetBSD:**

- NetBSD 9.0+

---

## Rollout Strategy

### Progressive Enablement

1. **Week 1-2:** Deploy Phase 1 features to staging
2. **Week 3-4:** Monitor metrics, tune parameters
3. **Week 5:** Deploy to production with kill switch
4. **Week 6+:** Gradual rollout of Phase 2 features

### Feature Flags

```toml
# Conservative profile (default)
[performance.socket]
recv_buffer_kb = 1024
send_buffer_kb = 1024

# Aggressive profile (high-performance)
[performance.socket]
recv_buffer_kb = 4096
send_buffer_kb = 4096

[performance.zerocopy]
use_sendfile = true
use_msg_zerocopy = true

[performance.batching]
enable_sendmmsg = true
max_batch_size = 64
```

### Monitoring & Rollback

**Key Metrics:**

- Transfer success rate (should remain >99.9%)
- Average transfer time (should decrease)
- CPU usage (should decrease)
- Memory usage (may increase slightly)
- Socket buffer drops (should decrease)

**Rollback Triggers:**

- Transfer success rate drops below 99%
- Increased error rates
- Memory leaks detected
- CPU usage increases unexpectedly

**Rollback Procedure:**

```bash
# Disable optimizations via config
sed -i 's/use_sendfile = true/use_sendfile = false/' tftp.toml
systemctl reload snow-owl-tftp
```

---

## Success Metrics

### Phase 1 Targets

- [ ] 50% reduction in packet drops under load
- [ ] 20% improvement in throughput
- [ ] Zero performance regression

### Phase 2 Targets

- [ ] 2x improvement in concurrent transfer capacity
- [ ] 30% reduction in CPU usage
- [ ] 60% reduction in syscall overhead

### Phase 3 Targets

- [ ] 3x improvement in maximum concurrent connections
- [ ] Sub-millisecond transfer initiation latency
- [ ] Linear scalability to 1000+ concurrent transfers

### Phase 4 Targets

- [ ] <10µs jitter for real-time deployments
- [ ] Zero packet loss under rated load
- [ ] Deterministic performance

---

## Documentation Requirements

### User Documentation

- [ ] Performance tuning guide
- [ ] Configuration examples for common scenarios
- [ ] Platform-specific recommendations
- [ ] Troubleshooting guide

### Developer Documentation

- [ ] Architecture decision records (ADRs)
- [ ] Performance optimization internals
- [ ] Benchmark methodology
- [ ] Profiling guide

### Operations Documentation

- [ ] Deployment best practices
- [ ] Monitoring and alerting setup
- [ ] Capacity planning guide
- [ ] Incident response procedures

---

## Risk Assessment

### High Risk

- **io_uring Integration:** Complex, may introduce bugs
  - *Mitigation:* Extensive testing, feature flag, fallback

- **MSG_ZEROCOPY:** Requires careful completion handling
  - *Mitigation:* Thorough testing, conservative thresholds

### Medium Risk

- **sendmmsg/recvmmsg:** Packet ordering considerations
  - *Mitigation:* Validate TFTP state machine compliance

- **Huge Pages:** May fail on systems without huge page support
  - *Mitigation:* Graceful fallback to normal pages

### Low Risk

- **Socket buffer tuning:** Well-understood, easy to revert
- **CPU affinity:** Minimal impact if misconfigured

---

## Open Questions

1. **Q:** Should we support automatic performance profile detection?
   **A:** TBD - evaluate in Phase 1

2. **Q:** What's the policy for balancing security vs. performance?
   **A:** Security defaults, performance opt-in

3. **Q:** Should we backport optimizations to macOS?
   **A:** Limited value (no sendfile for UDP, no sendmmsg)

4. **Q:** Integration with container orchestration (K8s)?
   **A:** Phase 4 consideration

5. **Q:** Support for hardware offload (TOE, RDMA)?
   **A:** Future research, very specialized

---

## References

### RFCs

- RFC 1350: The TFTP Protocol (Revision 2)
- RFC 2347: TFTP Option Extension
- RFC 2348: TFTP Blocksize Option
- RFC 2349: TFTP Timeout Interval and Transfer Size Options
- RFC 2090: TFTP Multicast Option

### Linux Kernel Documentation

- [sendfile(2)](https://man7.org/linux/man-pages/man2/sendfile.2.html)
- [sendmmsg(2)](https://man7.org/linux/man-pages/man2/sendmmsg.2.html)
- [io_uring](https://kernel.dk/io_uring.pdf)
- [MSG_ZEROCOPY](https://www.kernel.org/doc/html/latest/networking/msg_zerocopy.html)

### BSD Documentation

- [FreeBSD sendfile(2)](https://man.freebsd.org/cgi/man.cgi?query=sendfile)
- [OpenBSD sendmmsg(2)](https://man.openbsd.org/sendmmsg.2)

### Performance Resources

- [Optimizing UDP Performance](https://www.programmersought.com/article/89864415537/)
- [Linux Network Tuning Guide](https://wwwx.cs.unc.edu/~sparkst/howto/network_tuning.php)

---

## Changelog

| Version | Date       | Changes                                      |
|---------|------------|----------------------------------------------|
| 1.0     | 2026-01-19 | Initial roadmap created                      |

---

## Contact & Feedback

For questions, suggestions, or issues related to this roadmap:

- GitHub Issues: <https://github.com/192d-Wing/Snow-Owl/issues>
- Label: `performance` + `tftp`

---

**Status Legend:**

- ✅ Completed
- 🟡 In Progress
- ⚪ Not Started
- 🔬 Research Phase
- ⏸️ Deferred

**Priority Legend:**

- P0: Critical - Must have
- P1: High - Should have
- P2: Medium - Nice to have
- P3: Low - Future consideration
- P4: Research - Long-term exploration
