# TFTP Windowsize Testing Instructions

## Overview

We've created comprehensive RFC 7440 windowsize tests (tests 1-32) for the Snow-Owl TFTP implementation. This document provides instructions for running these tests.

## What Was Created

1. **windowsize-test.sh** - 32 comprehensive windowsize tests using atftp
2. **windowsize-analyzer.py** - Python-based performance analyzer
3. **run-all-tests.sh** - Master test runner for all test suites
4. **WINDOWSIZE_TESTS.md** - Detailed RFC 7440 documentation

## Prerequisites

### Install Required Tools

```bash
# Install atftp (required for windowsize tests)
sudo apt-get update
sudo apt-get install atftp

# Verify installation
atftp --version

# Ensure Python 3 is available (for analyzer)
python3 --version
```

### Build the TFTP Server

```bash
# Navigate to project root
cd /home/jwillman/projects/snow-owl

# Build the server
cargo build --release --bin snow-owl-tftp-server

# Verify binary exists
ls -lh target/release/snow-owl-tftp-server
```

## Running the Tests

### Option 1: Run All Tests (Recommended)

```bash
cd /home/jwillman/projects/snow-owl/crates/snow-owl-tftp
./tests/run-all-tests.sh
```

This will:
- Check prerequisites (atftp, md5sum, etc.)
- Build the project
- Run integration tests
- Run all 32 windowsize tests
- Display comprehensive summary

### Option 2: Run Only Windowsize Tests

```bash
cd /home/jwillman/projects/snow-owl/crates/snow-owl-tftp
./tests/windowsize-test.sh
```

Expected output:
```
================================================
  Snow-Owl TFTP Windowsize Tests (RFC 7440)
================================================

Setting up test environment...
Test environment ready

Starting TFTP server...
Server started (PID: 12345)

Running windowsize tests (1-32)...

Test 1: Windowsize 1 with small file (1KB)... ✓ PASS
Test 2: Windowsize 2 with small file (1KB)... ✓ PASS
...
Test 32: Windowsize 32 with exact window boundary... ✓ PASS

================================================
  Test Summary
================================================
Total:   32
Passed:  32
Failed:  0
Skipped: 0

All tests passed!
```

### Option 3: Python Performance Analyzer

```bash
cd /home/jwillman/projects/snow-owl/crates/snow-owl-tftp

# Quick test (windowsize 1-8 with medium file)
./tests/windowsize-analyzer.py quick

# Full test suite (all 32 tests)
./tests/windowsize-analyzer.py full

# Performance comparison (various windowsizes)
./tests/windowsize-analyzer.py performance
```

Expected output:
```
Snow-Owl TFTP Windowsize Analyzer (RFC 7440)
============================================================

Performance Test: Large file with various windowsizes

  Testing windowsize 1... ✓ 0.03 Mbps
  Testing windowsize 2... ✓ 0.07 Mbps
  Testing windowsize 4... ✓ 0.12 Mbps
  Testing windowsize 8... ✓ 0.21 Mbps
  Testing windowsize 16... ✓ 0.35 Mbps
  Testing windowsize 32... ✓ 0.52 Mbps
  Testing windowsize 64... ✓ 0.68 Mbps

====================================================================================================
WS   File Size    Time (s)   Throughput   Packets  ACKs     Retrans  Loss %
====================================================================================================
1    102400       2.731      0.03         200      200      0        0.00
2    102400       1.421      0.07         200      100      0        0.00
4    102400       0.721      0.12         200      50       0        0.00
8    102400       0.412      0.21         200      25       0        0.00
16   102400       0.248      0.35         200      13       0        0.00
32   102400       0.167      0.52         200      7        0        0.00
64   102400       0.127      0.68         200      4        0        0.00
====================================================================================================

Performance Summary:
  Baseline (WS=1): 0.03 Mbps
  Best (WS=64): 0.68 Mbps
  Improvement: 2166.7%
```

## Test Configuration

The windowsize tests use this TFTP configuration:

```toml
root_dir = "/tmp/tftp-windowsize-test-*/root"
bind_addr = "0.0.0.0:6970"
max_file_size_bytes = 10485760

[logging]
level = "info"
format = "text"
file = "/tmp/tftp-windowsize-test-*/logs/tftp.log"
audit_enabled = true

[write_config]
enabled = false

[multicast]
enabled = false

[rfc7440]
enabled = true
default_windowsize = 16
```

## Current Implementation Status

Based on the codebase review, the TFTP server has:

✅ **Windowsize support implemented**
- `PerformanceConfig::default_windowsize` - Default: 1 (RFC 1350 compatible)
- RFC 7440 sliding window protocol
- Valid range: 1-65535
- Configurable per deployment

✅ **Performance optimizations**
- Large block sizes (default 8192 bytes)
- Buffer pooling
- Platform-specific optimizations (Linux/BSD)

## Expected Test Results

### Small Windowsize (1-4)
- Lower throughput
- High ACK overhead
- Compatible with all clients
- Good for unreliable networks

### Medium Windowsize (8-16)
- Balanced performance
- Recommended for production
- Good for most networks
- 5-10x improvement over WS=1

### Large Windowsize (32-64)
- Maximum throughput
- Best for high-latency links
- May increase packet loss on poor networks
- 15-20x improvement over WS=1

## Troubleshooting

### atftp not found
```bash
sudo apt-get install atftp
```

### cargo not found
```bash
# Install Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
```

### Server fails to start
```bash
# Check if port is in use
sudo netstat -ulnp | grep 6970

# Check server logs
cat /tmp/tftp-windowsize-test-*/logs/tftp.log
```

### Tests timeout
- Increase timeout in test scripts
- Check firewall settings
- Verify server is running

### Permission denied
```bash
# Ensure scripts are executable
chmod +x tests/*.sh
chmod +x tests/*.py
```

## Next Steps After Testing

1. **Review Test Results**
   - Check for any failures
   - Analyze performance metrics
   - Identify optimal windowsize for your use case

2. **Update Configuration**
   - Adjust `default_windowsize` based on results
   - Tune for your network latency
   - Consider typical file sizes

3. **Production Deployment**
   - Set windowsize to 8-16 for general use
   - Use 32+ for high-latency networks
   - Keep at 1 for maximum compatibility

4. **Documentation**
   - Document chosen windowsize value
   - Record performance benchmarks
   - Update deployment guides

## Performance Tuning Recommendations

Based on network characteristics:

```toml
# Low latency local network (< 1ms RTT)
[performance]
default_windowsize = 8

# Internet / WAN (10-50ms RTT)
[performance]
default_windowsize = 16

# High latency satellite/international (100-300ms RTT)
[performance]
default_windowsize = 32

# Very high latency (> 300ms RTT)
[performance]
default_windowsize = 64
```

## References

- [RFC 7440 - TFTP Windowsize Option](https://tools.ietf.org/html/rfc7440)
- [RFC 1350 - The TFTP Protocol](https://tools.ietf.org/html/rfc1350)
- [WINDOWSIZE_TESTS.md](WINDOWSIZE_TESTS.md) - Detailed test documentation
- [README.md](README.md) - Main test suite documentation

## Support

For issues or questions:
- Check the troubleshooting section above
- Review server logs in `/tmp/tftp-windowsize-test-*/logs/`
- Ensure all prerequisites are installed
- Verify network connectivity and firewall settings
