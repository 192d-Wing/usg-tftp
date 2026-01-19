# Phase 3 Design Document
## io_uring Integration for Advanced Async I/O

### Executive Summary

Phase 3 introduces true asynchronous file I/O using io_uring, replacing tokio's thread pool-based file operations. This provides significant performance improvements for concurrent transfers and reduces memory overhead.

**Status:** Design Phase
**Target:** Linux 5.1+ (with graceful fallback)
**Expected Impact:** 50-100% improvement in concurrent transfer scalability

---

## Current Architecture Analysis

### Existing File I/O Pattern

**File:** `src/main.rs:1357-1660`

```rust
// Current: tokio::fs::File (uses blocking thread pool)
let mut file = File::open(&file_path).await?;
let bytes_read = file.read(&mut read_buffer).await?;
```

**Characteristics:**
- Uses `tokio::fs::File`
- Each read operation dispatches to blocking thread pool
- Thread pool size limits concurrent I/O operations
- Context switching overhead between tokio runtime and thread pool

**Performance Bottleneck:**
- With 100+ concurrent transfers, thread pool becomes saturated
- Each transfer requires dedicated thread pool slot
- Memory overhead: ~2MB stack per thread
- Context switching latency: ~5-10µs per read

---

## io_uring Architecture

### Overview

io_uring is a Linux kernel interface (5.1+) that provides true async I/O without thread pools.

**Key Benefits:**
1. **Zero-copy I/O** - Direct kernel memory access
2. **True async** - No blocking threads
3. **Batch operations** - Submit multiple operations in single syscall
4. **Polling mode** - Optional kernel-side polling for ultra-low latency

### Architecture Constraints

**Challenge:** tokio-uring is a separate runtime incompatible with tokio

**Options:**

#### Option 1: Dual Runtime (Recommended)
- Keep tokio for network I/O (UDP sockets)
- Add tokio-uring runtime for file I/O only
- Use channels (tokio::sync::mpsc) to coordinate

#### Option 2: Full Migration
- Replace entire runtime with tokio-uring
- Complex: tokio-uring doesn't support UDP sockets well
- Not recommended due to UDP limitations

#### Option 3: Hybrid with Feature Flag
- Make io_uring optional via cargo feature
- Runtime detection and fallback
- Best for gradual rollout

---

## Proposed Design: Dual Runtime with Feature Flag

### Architecture Diagram

```
┌─────────────────────────────────────────────────────────────┐
│                       Main Thread                            │
│                                                              │
│  ┌────────────────┐                  ┌──────────────────┐   │
│  │ Tokio Runtime  │                  │ io_uring Runtime │   │
│  │                │                  │  (Optional)      │   │
│  │ - UDP Socket   │◄────channel────►│  - File Reads    │   │
│  │ - Main Loop    │                  │  - File Writes   │   │
│  │ - Timers       │                  │                  │   │
│  └────────────────┘                  └──────────────────┘   │
│         │                                     │              │
│         ▼                                     ▼              │
│  Network I/O (async)              File I/O (true async)     │
└─────────────────────────────────────────────────────────────┘
```

### Implementation Strategy

#### Phase 3.1: Foundation (Week 1-2)

**Tasks:**
1. Add tokio-uring as optional dependency
2. Create file I/O abstraction trait
3. Implement fallback mechanism
4. Add configuration structure

**Files to Create:**
- `src/io_backend.rs` - I/O backend abstraction
- `src/io_uring_backend.rs` - io_uring implementation
- `src/tokio_backend.rs` - tokio::fs fallback

**Configuration:**
```toml
[performance.platform.io_uring]
enabled = false              # Default: disabled (opt-in)
queue_depth = 128            # io_uring queue size
use_sqpoll = false           # Kernel polling mode
sq_thread_cpu = null         # CPU for polling thread
fallback_on_error = true     # Auto-fallback to tokio
```

#### Phase 3.2: File Reading (Week 3-4)

**Implementation:**

```rust
// src/io_backend.rs
#[async_trait]
pub trait FileBackend: Send + Sync {
    async fn open(&self, path: &Path) -> Result<Box<dyn FileHandle>>;
    fn backend_name(&self) -> &str;
}

#[async_trait]
pub trait FileHandle: Send + Sync {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize>;
    async fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> Result<()>;
    async fn metadata(&self) -> Result<Metadata>;
}

// Factory function
pub fn create_file_backend(config: &IoUringConfig) -> Box<dyn FileBackend> {
    #[cfg(feature = "io_uring")]
    if config.enabled && is_io_uring_available() {
        return Box::new(IoUringBackend::new(config));
    }

    Box::new(TokioBackend::new())
}
```

**Integration Points:**
- `handle_read_request()` - Replace `File::open()` with backend
- `send_file_data_streaming()` - Use backend for reads

#### Phase 3.3: Testing & Benchmarking (Week 5)

**Test Plan:**
1. Unit tests for both backends
2. Integration tests with io_uring feature enabled/disabled
3. Stress test: 1000+ concurrent transfers
4. Benchmark comparison: tokio vs io_uring

**Metrics to Measure:**
- Throughput (MB/s)
- Concurrent transfer capacity
- CPU usage
- Memory usage
- Tail latency (p99, p999)

#### Phase 3.4: Write Support (Week 6, Optional)

**Scope:** Add io_uring for WRQ (write requests)
**Complexity:** Medium (requires careful error handling)
**Priority:** P2 (nice to have)

---

## Technical Challenges

### Challenge 1: Runtime Coordination

**Problem:** tokio-uring and tokio are separate runtimes

**Solution:**
```rust
// Spawn io_uring runtime in separate thread
let (tx, rx) = tokio::sync::mpsc::channel(100);

std::thread::spawn(move || {
    tokio_uring::start(async move {
        while let Some(request) = rx.recv().await {
            // Handle file I/O request
            let result = process_file_request(request).await;
            // Send result back via oneshot channel
        }
    });
});
```

### Challenge 2: Platform Detection

**Problem:** io_uring not available on all Linux systems

**Solution:**
```rust
fn is_io_uring_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        use std::fs::File;
        use std::os::unix::io::AsRawFd;

        // Try to create io_uring instance
        match io_uring::IoUring::new(1) {
            Ok(_) => {
                info!("io_uring available, Linux 5.1+ detected");
                true
            }
            Err(_) => {
                warn!("io_uring not available, falling back to tokio::fs");
                false
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    false
}
```

### Challenge 3: Buffer Lifecycle Management

**Problem:** io_uring requires buffers to remain valid during operation

**Solution:**
- Use reference-counted buffers (`Arc<Vec<u8>>`)
- Or use registered buffers (more complex but faster)
- Integrate with existing BufferPool

---

## Configuration Schema

```toml
[performance.platform.io_uring]
# Enable io_uring for file operations (Linux 5.1+ only)
# Requires: cargo build --features io_uring
# Default: false (opt-in)
enabled = false

# io_uring submission queue depth
# Higher values allow more concurrent operations
# Default: 128, Range: 1-4096
queue_depth = 128

# Enable SQPOLL mode (kernel-side polling)
# Reduces syscall overhead but uses dedicated kernel thread
# Recommended for high-throughput scenarios
# Default: false
use_sqpoll = false

# CPU for SQPOLL thread (if use_sqpoll = true)
# Pins kernel polling thread to specific CPU
# Default: null (no pinning)
sq_thread_cpu = null

# Auto-fallback to tokio::fs on error
# If io_uring operations fail, automatically use thread pool
# Default: true
fallback_on_error = true

# Use registered buffers for zero-copy
# More complex but eliminates buffer copies
# Default: false (experimental)
use_registered_buffers = false
```

---

## Performance Expectations

### Baseline (Current: tokio::fs)
- Concurrent transfers: ~200 (limited by thread pool)
- Memory per transfer: ~2MB (thread stack)
- Read latency: ~50-100µs (thread pool dispatch)
- CPU usage: Moderate (context switching overhead)

### Target (io_uring)
- Concurrent transfers: 1000+ (no thread pool limit)
- Memory per transfer: ~64KB (buffer only)
- Read latency: ~10-20µs (direct kernel access)
- CPU usage: 30-50% lower (fewer context switches)

### Expected Gains
| Metric | Current | io_uring | Improvement |
|--------|---------|----------|-------------|
| Max Concurrent | 200 | 1000+ | 5x |
| Memory/Transfer | 2MB | 64KB | 30x less |
| Read Latency | 50µs | 15µs | 3x faster |
| CPU Usage | 100% | 50-70% | 30-50% less |
| Throughput | 1GB/s | 2-3GB/s | 2-3x |

---

## Risks & Mitigations

### Risk 1: Increased Complexity
**Impact:** High
**Mitigation:**
- Feature flag for gradual rollout
- Extensive testing before production
- Maintain tokio::fs fallback indefinitely

### Risk 2: Platform Compatibility
**Impact:** Medium
**Mitigation:**
- Runtime detection with automatic fallback
- Clear documentation of requirements
- Works on Linux 5.1+, gracefully degrades elsewhere

### Risk 3: Bug in io_uring Runtime
**Impact:** Medium
**Mitigation:**
- Use stable tokio-uring version
- Monitor tokio-uring issue tracker
- Fallback option always available

---

## Alternative Approaches Considered

### Alternative 1: io-uring crate (not tokio-uring)
**Pros:** More direct control
**Cons:** No async/await support, harder to use
**Decision:** Rejected - too low-level

### Alternative 2: glommio
**Pros:** Thread-per-core architecture, good for high throughput
**Cons:** Different runtime model, requires full rewrite
**Decision:** Rejected - too invasive

### Alternative 3: Stay with tokio::fs
**Pros:** Simple, works well for current load
**Cons:** Limited scalability for >200 concurrent transfers
**Decision:** Keep as fallback, but implement io_uring for future growth

---

## Rollout Plan

### Stage 1: Development (Weeks 1-4)
- Implement feature flag
- Create abstraction layer
- Implement io_uring backend
- Unit tests

### Stage 2: Internal Testing (Week 5)
- Integration tests
- Stress testing with 1000+ concurrent transfers
- Performance benchmarking
- Bug fixes

### Stage 3: Beta (Week 6-8)
- Feature flag disabled by default
- Documentation for beta testers
- Gather feedback
- Monitor for issues

### Stage 4: Production (Week 9+)
- Enable by default on Linux 5.1+
- Monitor metrics
- Gradual rollout to production systems

---

## Open Questions

1. **Q:** Should we implement io_uring for writes (WRQ) in Phase 3?
   **A:** No - focus on reads first, writes in Phase 4

2. **Q:** Use registered buffers or regular buffers?
   **A:** Regular buffers first, registered as optimization later

3. **Q:** SQPOLL mode by default?
   **A:** No - requires careful CPU affinity tuning, opt-in only

4. **Q:** Minimum supported Linux version?
   **A:** Linux 5.1+ (io_uring baseline), recommend 5.10+ (LTS)

---

## Success Criteria

### Phase 3.1-3.2 Complete When:
- ✅ Feature flag compiles and tests pass
- ✅ Abstraction layer works with both backends
- ✅ io_uring backend reads files correctly
- ✅ Automatic fallback works on non-Linux

### Phase 3.3 Complete When:
- ✅ Stress test handles 1000+ concurrent transfers
- ✅ Performance benchmarks show 2x+ improvement
- ✅ No memory leaks detected
- ✅ CPU usage reduced by 30%+

### Production Ready When:
- ✅ Beta testing completed (no critical bugs)
- ✅ Documentation complete
- ✅ Monitoring dashboards ready
- ✅ Rollback procedure tested

---

## Conclusion

Phase 3 is a significant architectural enhancement that will dramatically improve scalability for high-concurrency scenarios. The dual-runtime approach with feature flags provides a safe, gradual migration path while maintaining backward compatibility.

**Recommendation:** Proceed with Phase 3 implementation after Phase 2 benchmarking confirms expected performance gains.

**Next Steps:**
1. Benchmark Phase 2 (recvmmsg/sendmmsg)
2. If Phase 2 meets targets, begin Phase 3.1
3. If not, optimize Phase 2 first before adding complexity
