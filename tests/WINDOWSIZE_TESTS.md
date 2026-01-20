# TFTP Windowsize Tests (RFC 7440)

This directory contains comprehensive tests for RFC 7440 windowsize option support in the Snow-Owl TFTP implementation.

## Test Files

### 1. `windowsize-test.sh`
Bash-based test suite covering all 32 windowsize test cases.

**Requirements:**
- `atftp` client (supports windowsize option)
- `md5sum` or `md5` (for file integrity verification)

**Installation:**
```bash
# Ubuntu/Debian
sudo apt-get install atftp

# macOS
brew install atftp
```

**Usage:**
```bash
# Run all 32 tests
cd crates/snow-owl-tftp
cargo build --release
./tests/windowsize-test.sh
```

**Test Coverage:**
- Tests 1-8: Small file (1 KB) with windowsize 1-8
- Tests 9-16: Medium file (10 KB) with windowsize 1, 2, 4, 8, 12, 16, 24, 32
- Tests 17-24: Large file (100 KB) with windowsize 1, 2, 4, 8, 16, 32, 48, 64
- Tests 25-28: XLarge file (512 KB) with windowsize 1, 8, 32, 64
- Tests 29-30: Single block file edge cases
- Tests 31-32: Exact window boundary edge cases

### 2. `windowsize-analyzer.py`
Python-based advanced testing tool with detailed performance metrics.

**Requirements:**
- Python 3.6+
- No external dependencies (uses standard library)

**Usage:**
```bash
# Quick test (windowsize 1-8 with medium file)
./tests/windowsize-analyzer.py quick

# Full test suite (all 32 tests)
./tests/windowsize-analyzer.py full

# Performance comparison (various windowsizes with large file)
./tests/windowsize-analyzer.py performance
```

**Metrics Collected:**
- File size transferred
- Transfer time
- Throughput (Mbps)
- Total packets sent
- Total ACKs sent
- Retransmissions
- Packet loss rate
- Average RTT

**Example Output:**
```
====================================================================================================
WS   File Size    Time (s)   Throughput   Packets  ACKs     Retrans  Loss %
====================================================================================================
1    10240        2.345      0.03         20       20       0        0.00
2    10240        1.234      0.07         20       10       0        0.00
4    10240        0.678      0.12         20       5        0        0.00
8    10240        0.398      0.21         20       3        0        0.00
====================================================================================================

Performance Summary:
  Baseline (WS=1): 0.03 Mbps
  Best (WS=8): 0.21 Mbps
  Improvement: 600.0%
```

## RFC 7440 Windowsize Option

### Overview
RFC 7440 defines the "windowsize" option for TFTP, which allows multiple DATA packets to be sent before requiring an ACK, significantly improving performance on high-latency networks.

### Specification
- **Option Name:** `windowsize`
- **Option Value:** Number of blocks (1-65535)
- **Default:** 1 (traditional TFTP stop-and-wait)
- **Negotiation:** Via OACK packet

### Protocol Flow

**Traditional TFTP (windowsize=1):**
```
Client -> Server: RRQ
Server -> Client: DATA (block 1)
Client -> Server: ACK (block 1)
Server -> Client: DATA (block 2)
Client -> Server: ACK (block 2)
...
```

**With Windowsize=4:**
```
Client -> Server: RRQ (windowsize=4)
Server -> Client: OACK (windowsize=4)
Client -> Server: ACK (block 0)
Server -> Client: DATA (block 1)
Server -> Client: DATA (block 2)
Server -> Client: DATA (block 3)
Server -> Client: DATA (block 4)
Client -> Server: ACK (block 4)
Server -> Client: DATA (block 5)
...
```

## Test Scenarios

### Edge Cases Tested

1. **Single Block Transfer**
   - File size < 512 bytes
   - Should complete in one DATA packet
   - Windowsize should not affect behavior

2. **Exact Window Boundary**
   - File size = windowsize × blocksize
   - Tests proper window boundary handling
   - No partial window at end

3. **Small Windowsize (1-8)**
   - Low throughput, high ACK overhead
   - Tests proper acknowledgment logic
   - Baseline for performance comparison

4. **Medium Windowsize (12-32)**
   - Balanced performance
   - Typical production values
   - Good for most networks

5. **Large Windowsize (48-64)**
   - Maximum throughput
   - Tests buffer management
   - May increase packet loss on poor networks

6. **Very Large Files**
   - Tests long-running transfers
   - Buffer management over time
   - Proper block number wrapping (if needed)

## Expected Results

### Performance Characteristics

**Throughput vs Windowsize (expected trend):**
- WS=1: Baseline (limited by RTT)
- WS=2: ~2x improvement
- WS=4: ~3-4x improvement
- WS=8: ~5-7x improvement
- WS=16: ~8-12x improvement
- WS=32: ~10-15x improvement
- WS=64: ~12-18x improvement (diminishing returns)

**Optimal Windowsize:**
```
optimal_windowsize = (bandwidth × RTT) / (blocksize × 8)
```

For localhost testing (RTT ≈ 0.1ms), performance gains plateau around windowsize=16-32.

### File Integrity
All tests verify MD5 checksums to ensure:
- No data corruption
- Correct block ordering
- Complete file transfer
- Proper handling of last block

## Implementation Requirements

The Snow-Owl TFTP server must:
1. Accept windowsize option in RRQ
2. Respond with OACK if windowsize is supported
3. Send multiple DATA packets per window
4. Wait for ACK of last block in window
5. Handle partial windows at end of file
6. Support windowsize range: 1-65535
7. Default to 1 if not negotiated

## Running the Tests

### Quick Test
```bash
# Build the server
cargo build --release

# Run quick windowsize test
./tests/windowsize-test.sh

# Or use Python analyzer for detailed metrics
./tests/windowsize-analyzer.py quick
```

### Full Test Suite
```bash
# Run all 32 tests
./tests/windowsize-test.sh

# Or with Python analyzer
./tests/windowsize-analyzer.py full
```

### Performance Analysis
```bash
# Run performance comparison
./tests/windowsize-analyzer.py performance
```

## Troubleshooting

### atftp not found
```bash
sudo apt-get install atftp  # Ubuntu/Debian
brew install atftp          # macOS
```

### Connection refused
- Ensure server is built: `cargo build --release`
- Check server is running
- Verify port 6970 is not in use
- Check firewall settings

### Test failures
- Check server logs: `/tmp/tftp-windowsize-test-*/logs/tftp.log`
- Verify test files were created
- Ensure sufficient disk space
- Check MD5 utility is available

### Timeout errors
- Increase timeout in test scripts
- Check network latency
- Verify server performance
- Consider reducing file sizes

## References

- [RFC 7440 - TFTP Windowsize Option](https://tools.ietf.org/html/rfc7440)
- [RFC 1350 - The TFTP Protocol (Revision 2)](https://tools.ietf.org/html/rfc1350)
- [RFC 2347 - TFTP Option Extension](https://tools.ietf.org/html/rfc2347)
- [RFC 2348 - TFTP Blocksize Option](https://tools.ietf.org/html/rfc2348)

## License

Part of the Snow-Owl project. See main LICENSE file.
