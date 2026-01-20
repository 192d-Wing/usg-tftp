# TFTP Server Integration Testing

This document describes integration testing procedures for the Snow-Owl TFTP server, with a focus on RFC compliance validation and interoperability with standard TFTP clients.

## Overview

Integration tests verify that the TFTP server correctly implements RFC standards and works with real-world TFTP clients including:

- **tftp-hpa** - Standard Linux TFTP client
- **curl** - Command-line HTTP/TFTP client
- **atftp** - Advanced TFTP client with option support
- **Windows TFTP** - Built-in Windows client

## Test Environment Setup

### Prerequisites

```bash
# Install TFTP clients (Ubuntu/Debian)
sudo apt-get install tftp-hpa atftp curl

# Install TFTP clients (macOS)
brew install tftp-hpa atftp curl

# Create test directory structure
mkdir -p /tmp/tftp-test/{root,client}
cd /tmp/tftp-test
```

### Server Configuration

Create a test configuration file `/tmp/tftp-test/tftp.toml`:

```toml
root_dir = "/tmp/tftp-test/root"
bind_addr = "[::]:6969"  # Use non-privileged port for testing
max_file_size_bytes = 10485760  # 10MB for testing

[logging]
level = "debug"
format = "text"
audit_enabled = true

[write_config]
enabled = true
allow_overwrite = true
allowed_patterns = [
    "*.txt",
    "*.bin",
    "*.cfg",
    "uploads/*"
]

[multicast]
enabled = false
```

### Test Data Files

```bash
# Create test files with known content
echo "Hello, TFTP!" > /tmp/tftp-test/root/hello.txt
dd if=/dev/urandom of=/tmp/tftp-test/root/random.bin bs=1024 count=100
printf "Line 1\nLine 2\nLine 3\n" > /tmp/tftp-test/root/lines.txt

# Create large file for transfer testing
dd if=/dev/zero of=/tmp/tftp-test/root/large.bin bs=1M count=5

# Create NETASCII test file with Unix line endings
printf "Unix\nLine\nEndings\n" > /tmp/tftp-test/root/unix-lines.txt
```

### Start Test Server

```bash
# Build and run the server
cargo build --release --manifest-path /path/to/Snow-Owl/Cargo.toml

# Run server (in separate terminal)
/path/to/Snow-Owl/target/release/snow-owl-tftp --config /tmp/tftp-test/tftp.toml
```

## RFC Compliance Tests

### Test 1: Basic RRQ (RFC 1350)

**Objective**: Verify basic read request functionality

```bash
cd /tmp/tftp-test/client

# Test with tftp-hpa
tftp localhost 6969 << EOF
mode octet
get hello.txt
quit
EOF

# Verify file content
cat hello.txt
# Expected: "Hello, TFTP!"

# Cleanup
rm hello.txt
```

**Expected Result**: File successfully downloaded with correct content

**Audit Log Check**:
```bash
# Should see: read_request, transfer_started, transfer_completed
grep -E "(read_request|transfer_completed)" /tmp/tftp-test/tftp.log
```

---

### Test 2: NETASCII Transfer Size (RFC 2349)

**Objective**: Verify accurate transfer size reporting for NETASCII mode after line ending conversion

```bash
cd /tmp/tftp-test/client

# Read file size before transfer
ORIG_SIZE=$(wc -c < /tmp/tftp-test/root/unix-lines.txt)

# Transfer in NETASCII mode
tftp localhost 6969 << EOF
mode netascii
get unix-lines.txt
quit
EOF

# Check received size
RECV_SIZE=$(wc -c < unix-lines.txt)

echo "Original size: $ORIG_SIZE bytes"
echo "Received size: $RECV_SIZE bytes"

# Cleanup
rm unix-lines.txt
```

**Expected Result**:
- Transfer completes successfully
- Server logs show correct transfer size including CR+LF conversion overhead
- Audit log `bytes_transferred` matches actual bytes sent over network

**Audit Log Check**:
```bash
# Look for transfer_completed event with accurate bytes_transferred
grep "transfer_completed" /tmp/tftp-test/tftp.log | grep "unix-lines.txt"
```

---

### Test 3: Block Size Negotiation (RFC 2348)

**Objective**: Verify RFC 2348 block size option negotiation

```bash
cd /tmp/tftp-test/client

# Test with atftp (supports options)
atftp --option "blksize 1024" --get -r large.bin -l large-1k.bin localhost 6969

# Test with invalid block size (should fall back to default 512)
atftp --option "blksize 4" --get -r large.bin -l large-default.bin localhost 6969

# Verify files
md5sum /tmp/tftp-test/root/large.bin large-1k.bin large-default.bin

# Cleanup
rm large-*.bin
```

**Expected Result**:
- 1024-byte blocks: Transfer uses negotiated block size
- 4-byte blocks (invalid): Server logs warning, uses default 512
- Both transfers complete successfully with correct data

**Server Log Check**:
```bash
# Should see warning about invalid blksize=4
grep "invalid blksize" /tmp/tftp-test/tftp.log
```

---

### Test 4: Transfer Size Option (RFC 2349)

**Objective**: Verify RFC 2349 transfer size option support

```bash
cd /tmp/tftp-test/client

# Use atftp with tsize option
atftp --option "tsize 0" --get -r random.bin -l random-tsize.bin localhost 6969

# Verify transfer
md5sum /tmp/tftp-test/root/random.bin random-tsize.bin

# Cleanup
rm random-tsize.bin
```

**Expected Result**:
- Client sends `tsize=0` in RRQ
- Server responds with actual file size in OACK
- Transfer completes successfully

**Packet Capture** (optional):
```bash
# Capture TFTP traffic
sudo tcpdump -i lo -n port 6969 -w /tmp/tftp-capture.pcap

# Analyze with Wireshark or tcpdump
tcpdump -r /tmp/tftp-capture.pcap -A | grep -A 5 "tsize"
```

---

### Test 5: Timeout and Retransmission (RFC 1350)

**Objective**: Verify timeout handling and error packet transmission

```bash
# This test requires packet dropping or client simulation
# Manual test: Start transfer and disconnect client mid-transfer

cd /tmp/tftp-test/client

# Start transfer
timeout 5s tftp localhost 6969 << EOF
mode octet
get large.bin
EOF

# Expected: Timeout after 5 seconds
```

**Expected Result**:
- Server logs timeout errors
- ERROR packets sent to client (visible in audit logs)

**Audit Log Check**:
```bash
# Should see timeout error or transfer_failed
grep -E "(timeout|transfer_failed)" /tmp/tftp-test/tftp.log
```

---

### Test 6: Duplicate ACK Handling (RFC 1350)

**Objective**: Verify duplicate ACK detection and retransmission

**Note**: This requires packet manipulation tools like `tc` (traffic control) or a custom test client. For manual testing, this behavior is validated through code review and unit tests.

**Automated Test** (requires custom test client):
```python
#!/usr/bin/env python3
# test_duplicate_ack.py - Send duplicate ACKs to test retransmission

import socket
import struct

SERVER = ('localhost', 6969)
FILE = 'random.bin'

sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)

# Send RRQ
rrq = struct.pack('!H', 1) + FILE.encode() + b'\x00' + b'octet\x00'
sock.sendto(rrq, SERVER)

# Receive first DATA packet
data, addr = sock.recvfrom(516)
opcode, block = struct.unpack('!HH', data[:4])

# Send ACK for block 1
ack = struct.pack('!HH', 4, block)
sock.sendto(ack, addr)

# Receive block 2
data, _ = sock.recvfrom(516)
opcode, block2 = struct.unpack('!HH', data[:4])

# Send duplicate ACK for block 1 (should trigger retransmission)
ack_dup = struct.pack('!HH', 4, 1)
sock.sendto(ack_dup, addr)

# Should receive retransmission of block 2
data_retrans, _ = sock.recvfrom(516)
opcode_retrans, block_retrans = struct.unpack('!HH', data_retrans[:4])

if block_retrans == block2:
    print("✓ Duplicate ACK triggered retransmission")
else:
    print("✗ Retransmission failed")

sock.close()
```

**Expected Result**:
- Server detects duplicate ACK
- Logs: "Duplicate ACK detected, retransmitting block X"
- Retransmits current DATA packet

---

### Test 7: Invalid Option Negotiation (RFC 2347)

**Objective**: Verify server handles invalid option values gracefully

```bash
# Use atftp with invalid timeout value
atftp --option "timeout 999" --get -r hello.txt -l hello-badtimeout.bin localhost 6969

# Use atftp with non-numeric blksize
# (requires custom TFTP client or packet crafting)

# Verify transfer completes with defaults
cat hello-badtimeout.bin

# Cleanup
rm hello-badtimeout.bin
```

**Expected Result**:
- Server logs warning: "invalid timeout=999 (valid: 1-255)"
- Server omits invalid option from OACK
- Transfer completes with default timeout value

**Server Log Check**:
```bash
grep "invalid timeout" /tmp/tftp-test/tftp.log
grep "non-numeric" /tmp/tftp-test/tftp.log
```

---

## Write Operation Tests

### Test 8: Basic WRQ (RFC 1350)

**Objective**: Verify basic write request functionality

```bash
cd /tmp/tftp-test/client

# Create test file
echo "Upload test content" > upload.txt

# Upload with tftp-hpa
tftp localhost 6969 << EOF
mode octet
put upload.txt
quit
EOF

# Verify file on server
cat /tmp/tftp-test/root/upload.txt
# Expected: "Upload test content"

# Cleanup
rm upload.txt
rm /tmp/tftp-test/root/upload.txt
```

**Expected Result**: File uploaded successfully with correct content

**Audit Log Check**:
```bash
grep -E "(write_request|write_completed)" /tmp/tftp-test/tftp.log
```

---

### Test 9: Write with Pattern Validation

**Objective**: Verify pattern-based write access control

```bash
cd /tmp/tftp-test/client

# Test allowed pattern (*.txt)
echo "Allowed" > allowed.txt
tftp localhost 6969 << EOF
mode octet
put allowed.txt
quit
EOF
# Expected: Success

# Test disallowed pattern (*.exe)
echo "Blocked" > blocked.exe
tftp localhost 6969 << EOF
mode octet
put blocked.exe
quit
EOF
# Expected: Error "File not allowed for writing"

# Test upload to allowed subdirectory
echo "Upload content" > subdir-upload.txt
tftp localhost 6969 << EOF
mode octet
put subdir-upload.txt uploads/file.txt
quit
EOF
# Expected: Success

# Cleanup
rm allowed.txt blocked.exe subdir-upload.txt
rm /tmp/tftp-test/root/allowed.txt
rm /tmp/tftp-test/root/uploads/file.txt
```

**Expected Result**:
- allowed.txt: Upload succeeds
- blocked.exe: Server returns ERROR, logs "file not in allowed_patterns"
- uploads/file.txt: Upload succeeds

---

### Test 10: Write with Transfer Size Validation

**Objective**: Verify transfer size validation for WRQ

```bash
cd /tmp/tftp-test/client

# Create 1KB file
dd if=/dev/zero of=test-1k.bin bs=1024 count=1

# Upload with tsize option
atftp --option "tsize 1024" --put -l test-1k.bin -r test-1k.bin localhost 6969

# Verify upload
diff /tmp/tftp-test/client/test-1k.bin /tmp/tftp-test/root/test-1k.bin

# Test size mismatch (requires custom client to send wrong tsize)
# Expected: Server logs warning but completes transfer

# Cleanup
rm test-1k.bin /tmp/tftp-test/root/test-1k.bin
```

**Expected Result**:
- Transfer completes successfully
- If size matches: No warnings
- If size mismatches: Warning logged, file still written

---

## Performance Tests

### Test 11: Large File Transfer

**Objective**: Verify throughput and performance metrics

```bash
cd /tmp/tftp-test/client

# Create 10MB file
dd if=/dev/zero of=/tmp/tftp-test/root/large-10m.bin bs=1M count=10

# Measure download time
time tftp localhost 6969 << EOF
mode octet
get large-10m.bin
quit
EOF

# Check audit log for throughput metrics
grep "large-10m.bin" /tmp/tftp-test/tftp.log | grep "throughput_bps"

# Cleanup
rm large-10m.bin /tmp/tftp-test/root/large-10m.bin
```

**Expected Result**:
- Transfer completes in reasonable time
- Audit log shows throughput_bps metric
- No packet loss or retransmissions

---

### Test 12: Concurrent Transfers

**Objective**: Verify server handles multiple simultaneous clients

```bash
cd /tmp/tftp-test/client

# Launch 5 concurrent downloads
for i in {1..5}; do
  (
    tftp localhost 6969 << EOF
mode octet
get random.bin random-$i.bin
quit
EOF
  ) &
done

# Wait for all transfers
wait

# Verify all files
for i in {1..5}; do
  md5sum random-$i.bin
done

# Cleanup
rm random-*.bin
```

**Expected Result**:
- All 5 transfers complete successfully
- All files have matching checksums
- No server errors or crashes

---

## Security Tests

### Test 13: Path Traversal Prevention

**Objective**: Verify directory traversal protection

```bash
cd /tmp/tftp-test/client

# Attempt path traversal
tftp localhost 6969 << EOF
mode octet
get ../../../etc/passwd
quit
EOF
# Expected: Error "Access violation"

# Attempt with encoded characters
tftp localhost 6969 << EOF
mode octet
get ..%2F..%2F..%2Fetc%2Fpasswd
quit
EOF
# Expected: Error "Access violation"
```

**Expected Result**:
- Both attempts rejected with "Access violation"
- Audit log shows "path_traversal_attempt"

**Audit Log Check**:
```bash
grep "path_traversal_attempt" /tmp/tftp-test/tftp.log
```

---

### Test 14: Write Access Control

**Objective**: Verify write operations can be disabled

```bash
# Stop server and modify config
# Set write_config.enabled = false in tftp.toml

# Restart server

cd /tmp/tftp-test/client
echo "Test" > disabled-write.txt

tftp localhost 6969 << EOF
mode octet
put disabled-write.txt
quit
EOF
# Expected: Error "Write not supported"

# Cleanup
rm disabled-write.txt
```

**Expected Result**:
- Write request denied
- Audit log: "write_request_denied" with reason "writes disabled"

---

## IPv6 Tests

### Test 11: IPv6 Basic RRQ

**Objective**: Verify basic IPv6 read request functionality

```bash
# Requires atftp for IPv6 support
atftp -g -r hello.txt -l hello-ipv6.txt ::1 6970

# Verify file content
cat hello-ipv6.txt
# Expected: "Hello, TFTP!"
```

**Expected Result**: File successfully downloaded over IPv6 with correct content

---

### Test 12: IPv6 Large File Transfer

**Objective**: Verify IPv6 file integrity for larger transfers

```bash
atftp -g -r random.bin -l random-ipv6.bin ::1 6970

# Verify checksum
md5sum /tmp/tftp-test/root/random.bin random-ipv6.bin
```

**Expected Result**: File transfers complete with matching checksums

---

### Test 13: IPv6 Write Request

**Objective**: Verify IPv6 write request functionality

```bash
echo "IPv6 upload test content" > upload-ipv6.txt
atftp -p -l upload-ipv6.txt -r upload-ipv6.txt ::1 6970

# Verify uploaded file
cat /tmp/tftp-test/root/upload-ipv6.txt
```

**Expected Result**: File uploaded successfully over IPv6

---

### Test 14: IPv6 Dual-Stack

**Objective**: Verify server bound to [::] accepts IPv4 clients

```bash
# Server config: bind_addr = "[::]:6970"
# Connect with IPv4 client to dual-stack server
tftp 127.0.0.1 6970 << EOF
mode octet
get hello.txt hello-dualstack.txt
quit
EOF

cat hello-dualstack.txt
```

**Expected Result**: IPv4 client can connect to IPv6 dual-stack server

---

## Client Compatibility Matrix

| Client | RRQ | WRQ | Options | Block Size | Transfer Size | Notes |
|--------|-----|-----|---------|------------|---------------|-------|
| tftp-hpa | ✓ | ✓ | ✗ | 512 only | ✗ | Standard client |
| atftp | ✓ | ✓ | ✓ | Configurable | ✓ | Full option support |
| curl | ✓ | ✓ | ✗ | 512 only | ✗ | HTTP-style interface |
| Windows TFTP | ✓ | ✓ | ✗ | 512 only | ✗ | Built-in client |
| iPXE | ✓ | ✗ | Partial | Configurable | ✓ | Network boot |

---

## Automated Test Script

```bash
#!/bin/bash
# integration-test.sh - Automated TFTP integration tests

set -e

TEST_DIR="/tmp/tftp-test"
SERVER_PORT="6969"
RESULTS_FILE="$TEST_DIR/test-results.txt"

echo "Snow-Owl TFTP Integration Tests" > "$RESULTS_FILE"
echo "===============================" >> "$RESULTS_FILE"
echo "" >> "$RESULTS_FILE"

run_test() {
    local test_name="$1"
    local test_cmd="$2"

    echo -n "Running: $test_name... "

    if eval "$test_cmd" >> "$RESULTS_FILE" 2>&1; then
        echo "✓ PASS"
        echo "✓ $test_name: PASS" >> "$RESULTS_FILE"
    else
        echo "✗ FAIL"
        echo "✗ $test_name: FAIL" >> "$RESULTS_FILE"
    fi
    echo "" >> "$RESULTS_FILE"
}

# Test 1: Basic read
run_test "Test 1: Basic RRQ" "cd $TEST_DIR/client && tftp localhost $SERVER_PORT << EOF
mode octet
get hello.txt
quit
EOF
cat hello.txt | grep -q 'Hello, TFTP!'"

# Test 2: Basic write
run_test "Test 2: Basic WRQ" "cd $TEST_DIR/client && echo 'test' > test.txt && tftp localhost $SERVER_PORT << EOF
mode octet
put test.txt
quit
EOF
cat $TEST_DIR/root/test.txt | grep -q 'test'"

# Test 3: Large file
run_test "Test 3: Large file transfer" "cd $TEST_DIR/client && tftp localhost $SERVER_PORT << EOF
mode octet
get large.bin
quit
EOF
md5sum large.bin $TEST_DIR/root/large.bin | uniq | wc -l | grep -q 1"

# Add more tests...

echo ""
echo "Test Results Summary:"
grep -E "✓|✗" "$RESULTS_FILE" | tail -n +2

# Show failures
FAILURES=$(grep "✗" "$RESULTS_FILE" | wc -l)
if [ "$FAILURES" -gt 0 ]; then
    echo ""
    echo "Failed tests:"
    grep "✗" "$RESULTS_FILE"
    exit 1
else
    echo ""
    echo "All tests passed!"
    exit 0
fi
```

Make the script executable:
```bash
chmod +x integration-test.sh
```

---

## Continuous Integration

### GitHub Actions Workflow

```yaml
# .github/workflows/tftp-integration-tests.yml
name: TFTP Integration Tests

on:
  push:
    branches: [ main ]
    paths:
      - 'crates/snow-owl-tftp/**'
  pull_request:
    branches: [ main ]

jobs:
  integration-test:
    runs-on: ubuntu-latest

    steps:
    - uses: actions/checkout@v3

    - name: Install Rust
      uses: actions-rs/toolchain@v1
      with:
        toolchain: stable
        override: true

    - name: Install TFTP clients
      run: |
        sudo apt-get update
        sudo apt-get install -y tftp-hpa atftp

    - name: Build server
      run: cargo build --release --manifest-path crates/snow-owl-tftp/Cargo.toml

    - name: Setup test environment
      run: |
        mkdir -p /tmp/tftp-test/{root,client}
        echo "Hello, TFTP!" > /tmp/tftp-test/root/hello.txt
        dd if=/dev/urandom of=/tmp/tftp-test/root/random.bin bs=1024 count=100

    - name: Run server
      run: |
        ./target/release/snow-owl-tftp --config crates/snow-owl-tftp/docs/test-config.toml &
        sleep 2

    - name: Run integration tests
      run: ./crates/snow-owl-tftp/docs/integration-test.sh

    - name: Upload test results
      if: always()
      uses: actions/upload-artifact@v3
      with:
        name: test-results
        path: /tmp/tftp-test/test-results.txt
```

---

## Troubleshooting

### Server Not Responding

```bash
# Check if server is running
ps aux | grep snow-owl-tftp

# Check port binding
sudo netstat -ulnp | grep 6969

# Check firewall
sudo ufw status | grep 6969
```

### Transfer Failures

```bash
# Enable debug logging
export RUST_LOG=debug

# Check audit logs
tail -f /tmp/tftp-test/tftp.log | grep -E "(error|warn|fail)"

# Packet capture
sudo tcpdump -i lo -n port 6969 -X
```

### Permission Errors

```bash
# Check directory permissions
ls -la /tmp/tftp-test/root

# Fix permissions
chmod 755 /tmp/tftp-test/root
chmod 644 /tmp/tftp-test/root/*
```

---

## References

- [RFC 1350 - The TFTP Protocol (Revision 2)](https://tools.ietf.org/html/rfc1350)
- [RFC 2347 - TFTP Option Extension](https://tools.ietf.org/html/rfc2347)
- [RFC 2348 - TFTP Blocksize Option](https://tools.ietf.org/html/rfc2348)
- [RFC 2349 - TFTP Timeout Interval and Transfer Size Options](https://tools.ietf.org/html/rfc2349)
- [RFC Compliance Improvements](./rfc-compliance-improvements.md)
- [Write Operations Documentation](./write-operations.md)
