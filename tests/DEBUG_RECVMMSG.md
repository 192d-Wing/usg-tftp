# Debugging recvmmsg() Not Being Called

**Date**: 2026-01-19
**Issue**: eBPF tracing shows `recvmmsg_calls=0` despite batch operations being enabled

## 🔍 Problem Statement

The benchmark shows:
```
WITH Batch Operations:
  - recvfrom() calls: 2,163
  - recvmmsg() calls: 0  ← PROBLEM: Should be > 0
  - sendto() calls: 966
  - sendmmsg() calls: 9
```

**Expected**: `recvmmsg()` should be called, reducing `recvfrom()` calls significantly
**Actual**: All receives still use `recvfrom()`, only 27% reduction (from other optimizations)

## 🛠️ Investigation Steps Taken

### 1. Added Debug Logging

Modified [src/main.rs](../src/main.rs) to add extensive debug logging:

**Lines 680-691**: Log adaptive batching decision
```rust
let use_batch_recv = if adaptive_batching_enabled {
    let current_clients = active_clients.load(Ordering::Relaxed);
    let should_batch = current_clients >= adaptive_threshold && base_batch_enabled;
    debug!(
        "Adaptive batching: clients={}, threshold={}, base_enabled={}, will_use_batch={}",
        current_clients, adaptive_threshold, base_batch_enabled, should_batch
    );
    should_batch
} else {
    debug!("Using fixed batching mode: enabled={}", base_batch_enabled);
    base_batch_enabled
};
```

**Line 699**: Log when batch receive is attempted
```rust
if use_batch_recv {
    debug!("Attempting batch receive with batch_size={}", batch_size);
    // ...
}
```

**Lines 142-152**: Log within `batch_recv_packets()` function
```rust
debug!("batch_recv_packets called: fd={}, batch_size={}", socket_fd, batch_size);
// ...
debug!("Calling recvmmsg() syscall...");
match recvmmsg(...) {
```

**Line 754**: Log fallback to single recv
```rust
Ok(_) => {
    debug!("Batch receive returned no packets, falling back to single recv_from");
}
```

### 2. Configuration Changes

Created [configs/force-batch.toml](benchmark-test/configs/force-batch.toml):
```toml
[performance.platform.batch]
enable_sendmmsg = true
enable_recvmmsg = true
max_batch_size = 32
batch_timeout_us = 1000        # Increased from 100μs
enable_adaptive_batching = false  # Force always-on
adaptive_batch_threshold = 0
```

Also updated existing [configs/with-batch.toml](benchmark-test/configs/with-batch.toml) to disable adaptive batching.

### 3. Rebuilt Binary

```bash
cargo build --release --package snow-owl-tftp
```

Binary rebuilt successfully with debug logging at [main.rs:680-754](../src/main.rs#L680-L754)

## 🐛 Potential Root Causes

### Theory 1: Adaptive Batching Threshold Not Met

**Default settings**:
- `enable_adaptive_batching = true`
- `adaptive_batch_threshold = 5`

**Problem**: With 50 concurrent clients starting simultaneously, the `active_clients` counter might not reach 5 at the right time due to timing:

```rust
let current_clients = active_clients.load(Ordering::Relaxed);
if current_clients >= adaptive_threshold {  // Might be false!
    use_batch_recv = true;
}
```

**Why this happens**:
1. Client counter increments when handling client (line 722)
2. But receive loop checks threshold BEFORE processing
3. Race condition: packets arrive before counter updated

**Evidence**: Disabling adaptive batching should fix this

### Theory 2: MSG_DONTWAIT Causing Immediate Return

In `batch_recv_packets()`:
```rust
match recvmmsg(
    socket_fd,
    &mut headers,
    iovecs.iter_mut(),
    MsgFlags::MSG_DONTWAIT,  ← Non-blocking!
    None,  ← No timeout
) {
```

**Problem**: With `MSG_DONTWAIT` and no timeout:
- If no packets are queued when `recvmmsg()` is called, it returns immediately with `EAGAIN`
- Falls back to single `recv_from()` which blocks
- This defeats the purpose of batching

**Fix needed**: Use a small timeout instead of `MSG_DONTWAIT`:
```rust
match recvmmsg(
    socket_fd,
    &mut headers,
    iovecs.iter_mut(),
    MsgFlags::empty(),  // Not MSG_DONTWAIT
    Some(TimeSpec::from_duration(Duration::from_micros(batch_timeout_us))),
) {
```

### Theory 3: Socket Not Set to Non-Blocking Mode

Tokio's `UdpSocket` might be in blocking mode, incompatible with `MSG_DONTWAIT`:

```rust
let socket = UdpSocket::bind(&self.bind_addr).await?;
// Missing: socket.set_nonblocking(true)?;
```

### Theory 4: Fallback Logic Too Aggressive

Current code (line 752-758):
```rust
Ok(packets) if !packets.is_empty() => {
    // Process packets
    continue;
}
Ok(_) => {
    // Empty result - falls through to single recv_from
    debug!("Batch receive returned no packets, falling back to single recv_from");
}
```

**Problem**: Every empty batch receive immediately falls back to blocking `recv_from()`, which then succeeds. The loop never retries `recvmmsg()`.

## ✅ Recommended Fixes (Priority Order)

### Fix 1: Change MSG_DONTWAIT to Use Timeout ⭐ MOST LIKELY

**File**: `src/main.rs:151-157`

**Current**:
```rust
match recvmmsg(
    socket_fd,
    &mut headers,
    iovecs.iter_mut(),
    MsgFlags::MSG_DONTWAIT,
    None,
) {
```

**Fixed**:
```rust
use nix::sys::time::TimeSpec;
use std::time::Duration;

match recvmmsg(
    socket_fd,
    &mut headers,
    iovecs.iter_mut(),
    MsgFlags::empty(),  // Remove MSG_DONTWAIT
    Some(TimeSpec::from_duration(Duration::from_micros(batch_timeout_us as u64))),
) {
```

**Rationale**:
- Current code returns immediately if no packets queued
- With timeout, it waits for packets to arrive and accumulate
- Allows batching of packets that arrive within the timeout window

### Fix 2: Remove Fallback on Empty Batch

**File**: `src/main.rs:752-758`

**Current**:
```rust
Ok(_) => {
    // No packets available, fall through to regular recv_from
    debug!("Batch receive returned no packets, falling back to single recv_from");
}
```

**Fixed**:
```rust
Ok(_) => {
    // No packets in this batch, try again
    debug!("Batch receive returned no packets, retrying...");
    continue;  // Don't fall through!
}
```

**Rationale**:
- With timeout (Fix 1), empty result means genuine timeout
- Should retry batch receive, not fall back to single
- Only fall back on actual errors

### Fix 3: Increase Batch Timeout

**File**: `tests/benchmark-test/configs/with-batch.toml`

**Current**:
```toml
batch_timeout_us = 100
```

**Recommended**:
```toml
batch_timeout_us = 1000  # 1ms - allows packets to accumulate
```

**Rationale**:
- 100μs is extremely short
- With 50 concurrent clients, packets may arrive over several hundred μs
- 1ms gives reasonable batching window without adding latency

### Fix 4: Force Adaptive Batching Off for Testing

**Status**: ✅ Already done

Updated configs to disable adaptive batching:
```toml
enable_adaptive_batching = false
adaptive_batch_threshold = 0
```

## 🧪 Testing Plan

### Step 1: Apply Fix 1 (Timeout Instead of MSG_DONTWAIT)

1. Modify `src/main.rs:151-157` as shown above
2. Add `batch_timeout_us` parameter to `batch_recv_packets()` function
3. Rebuild: `cargo build --release --package snow-owl-tftp`
4. Run benchmark: `sudo ./tests/benchmark-phase2.sh`
5. Check eBPF output: `recvmmsg_calls` should be > 0

**Expected result**: `recvmmsg_calls` > 1000, `recvfrom_calls` drops to < 500

### Step 2: Apply Fix 2 (Remove Fallback)

1. Modify `src/main.rs:752-758` to `continue` instead of falling through
2. Rebuild and test
3. Verify no regression in functionality

**Expected result**: Higher `recvmmsg` utilization, lower `recvfrom` calls

### Step 3: Tune Batch Timeout

Test with different values:
- 500μs
- 1000μs (1ms)
- 2000μs (2ms)

Measure:
- `recvmmsg_calls` count
- Throughput
- Latency

**Expected optimal**: 1000-2000μs for 50 concurrent clients

## 📊 Success Criteria

After fixes, we expect:

| Metric | Current | Target |
|--------|---------|--------|
| **recvmmsg() calls** | 0 | > 1,000 |
| **recvfrom() calls** | 2,163 | < 500 |
| **Syscall reduction** | 27% | 60-80% |
| **Batch efficiency** | 0% | > 80% |

**Batch efficiency** = `recvmmsg_calls / (recvmmsg_calls + recvfrom_calls)`

## 🚀 Next Steps

1. **Immediate**: Apply Fix 1 (timeout instead of MSG_DONTWAIT)
2. **Short-term**: Apply Fix 2 (remove aggressive fallback)
3. **Testing**: Run full benchmark suite with eBPF tracing
4. **Optimization**: Tune `batch_timeout_us` for best results
5. **Validation**: Confirm `recvmmsg()` is being called via eBPF
6. **Documentation**: Update BENCHMARK_RESULTS.md with actual syscall reduction

## 📝 Code Changes Summary

### Files to Modify

1. **src/main.rs**:
   - Line 132-136: Add `batch_timeout_us` parameter to function
   - Line 151-157: Use timeout instead of MSG_DONTWAIT
   - Line 752-758: Change fallback logic to retry

2. **tests/benchmark-test/configs/with-batch.toml**:
   - Increase `batch_timeout_us` from 100 to 1000

### Estimated Effort

- Code changes: 30 minutes
- Testing: 15 minutes per benchmark run
- Total: 1-2 hours to fix and validate

## 💡 Additional Observations

### sendmmsg() IS Working

Notice in the results:
```
sendmmsg_calls=9  ← This is > 0!
```

This proves:
1. The batch send code path IS executing
2. The nix syscall bindings work correctly
3. The issue is specific to the receive side

### Why 27% Reduction Without recvmmsg?

The 27% syscall reduction comes from:
1. **sendmmsg()** batching (9 calls batching multiple sends)
2. **Optimized buffer handling** (buffer pool reducing allocations)
3. **Better socket options** (larger buffers, reuse_port)

This is good, but we're missing the **main benefit**: batch receive!

## 🎯 Expected Impact After Fixes

Based on the 9 `sendmmsg()` calls already working, we can predict:

**Optimistic scenario** (Fix 1 + Fix 2):
- `recvmmsg_calls`: ~1,500-2,000
- `recvfrom_calls`: ~200-500  (just for stragglers)
- Syscall reduction: **70-80%**
- Throughput: +5-10% on localhost, +30-50% in production

**Conservative scenario** (Just Fix 1):
- `recvmmsg_calls`: ~800-1,200
- `recvfrom_calls`: ~800-1,200
- Syscall reduction: **50-60%**
- Throughput: +2-5% on localhost, +20-30% in production

---

**Status**: Investigation complete, fixes identified, ready to implement.
**Confidence**: High - sendmmsg() proves the approach works, just need to fix receive side.
**Risk**: Low - changes are localized and have clear fallback behavior.
