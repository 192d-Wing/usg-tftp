# Phase 2 Benchmarking Guide

## Overview

The `benchmark-phase2.sh` script provides comprehensive performance testing for Phase 2 batch operations (recvmmsg/sendmmsg) in the snow-owl-tftp server.

## Quick Start

### Running on Debian Trixie Container

```bash
# From container
cd /path/to/Snow-Owl/crates/snow-owl-tftp
sudo ./benchmark-phase2.sh
```

### Running on Host System

```bash
# Requires root for strace
sudo ./benchmark-phase2.sh
```

## Prerequisites

The script will automatically check for and install (on Debian/Ubuntu):
- `cargo` - Rust toolchain (must be pre-installed)
- `tftp` - TFTP client for testing
- `strace` - System call tracer for measuring syscall overhead
- `bc` - Calculator for performance metrics

**Minimum Requirements:**
- Linux 2.6.33+ (for recvmmsg support)
- Root privileges or CAP_SYS_PTRACE for strace

## What It Tests

### 1. Syscall Overhead Comparison

Tests the primary improvement of Phase 2: reduction in syscall count.

**Test Procedure:**
- Starts server with batch operations **disabled**
- Performs concurrent transfers under strace
- Counts recvfrom/sendto calls
- Repeats with batch operations **enabled**
- Counts recvmmsg usage

**Target:** 60-80% reduction in syscall count

### 2. Throughput Test

Measures raw transfer performance.

**Test Procedure:**
- Single large file (10 MB) transfer
- Measures MB/s throughput
- Compares with and without batch operations

**Target:** Visible improvement in throughput

### 3. Concurrent Transfer Test

Measures performance under concurrent load.

**Test Procedure:**
- 10 concurrent clients transferring 100 KB files
- Measures total completion time
- Compares with and without batch operations

**Target:** 2x improvement (50% time reduction)

## Output

### Terminal Output

The script provides colored, real-time progress:
- ✓ Success messages in green
- ✗ Error messages in red
- ℹ Info messages in blue
- Section headers in yellow

### Generated Files

All results are stored in `benchmark-test/results/`:

```
benchmark-test/
├── results/
│   ├── benchmark-report.txt       # Main comparison report
│   ├── syscalls-no-batch.txt      # strace output without batch
│   ├── syscalls-with-batch.txt    # strace output with batch
│   ├── metrics-no-batch.txt       # Parsed metrics without batch
│   ├── metrics-with-batch.txt     # Parsed metrics with batch
│   ├── server-no-batch.log        # Server log without batch
│   └── server-with-batch.log      # Server log with batch
├── configs/
│   ├── no-batch.toml              # Test config with batch disabled
│   └── with-batch.toml            # Test config with batch enabled
└── tftp-root/
    ├── test-1kb.bin               # Small test file
    ├── test-100kb.bin             # Medium test file
    └── test-10mb.bin              # Large test file
```

### Benchmark Report

The main report (`benchmark-report.txt`) includes:

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  Snow-Owl TFTP Phase 2 Benchmark Results
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Syscall Overhead Comparison:
  - WITHOUT batch: X recvfrom calls
  - WITH batch: Y recvmmsg calls
  - Reduction: Z%
  - Status: ✓ PASS / ✗ FAIL

Throughput Comparison:
  - Single file: X MB/s → Y MB/s (Z% improvement)
  - Concurrent: X seconds → Y seconds (Z% improvement)

Conclusion:
  - Performance assessment
  - Recommendations for Phase 3
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## Success Criteria

Phase 2 is considered successful if:

- ✅ **Syscall reduction:** 60%+ fewer recvfrom/sendto calls
- ✅ **Concurrent performance:** 2x improvement (50%+ faster)
- ✅ **Functionality:** All transfers complete successfully
- ✅ **Compatibility:** Graceful fallback on older kernels

## Customization

Edit the script variables to adjust test parameters:

```bash
# At the top of benchmark-phase2.sh
SERVER_PORT=6969              # Change test port
CONCURRENT_CLIENTS=10         # More concurrent clients for stress test
LARGE_FILE="test-10mb.bin"    # Larger files for throughput test
```

## Troubleshooting

### "strace: Operation not permitted"

**Solution:** Run with sudo or grant CAP_SYS_PTRACE:

```bash
sudo ./benchmark-phase2.sh
```

### "tftp: command not found"

**Solution:** Install TFTP client:

```bash
sudo apt-get update && sudo apt-get install -y tftp
```

### "cargo: command not found"

**Solution:** Install Rust toolchain:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
```

### Server fails to start

**Check:**
1. Port 6969 is not already in use: `netstat -ln | grep 6969`
2. Server logs in `benchmark-test/results/server-*.log`
3. Config files are valid TOML

### Transfers timeout

**Check:**
1. Firewall rules allow UDP port 6969
2. Server is actually running: `ps aux | grep snow-owl-tftp`
3. Increase timeout or reduce file sizes

## Integration with CI/CD

### GitHub Actions Example

```yaml
name: Phase 2 Benchmark

on: [push, pull_request]

jobs:
  benchmark:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - name: Install dependencies
        run: |
          sudo apt-get update
          sudo apt-get install -y tftp strace bc
      - name: Run benchmark
        run: |
          cd crates/snow-owl-tftp
          sudo ./benchmark-phase2.sh
      - name: Upload results
        uses: actions/upload-artifact@v3
        with:
          name: benchmark-results
          path: crates/snow-owl-tftp/benchmark-test/results/
```

## Next Steps After Benchmarking

### If Phase 2 Meets Targets

1. **Production rollout** - Deploy with batch operations enabled
2. **Monitor metrics** - Collect real-world performance data
3. **Phase 3 planning** - Evaluate io_uring implementation need
4. **Documentation** - Update README with performance characteristics

### If Phase 2 Below Targets

1. **Tune batch_size** - Try 64, 128 instead of 32
2. **Adjust buffer_kb** - Increase socket buffers to 4096KB
3. **Profile bottlenecks** - Use perf/flamegraph to find issues
4. **Re-test** - Run benchmark again after tuning
5. **Defer Phase 3** - Don't add io_uring complexity until Phase 2 optimized

## Reference

- [IMPLEMENTATION_SUMMARY.md](IMPLEMENTATION_SUMMARY.md) - Phase 1 & 2 overview
- [PHASE2_NOTES.md](PHASE2_NOTES.md) - Phase 2 implementation details
- [PERFORMANCE_ROADMAP.md](PERFORMANCE_ROADMAP.md) - Complete optimization roadmap
- [RFC 1350](https://tools.ietf.org/html/rfc1350) - TFTP Protocol Specification

## Support

For issues or questions:
1. Check server logs in `benchmark-test/results/`
2. Review strace output for syscall patterns
3. Verify kernel version supports recvmmsg (2.6.33+)
4. Ensure proper permissions for strace
