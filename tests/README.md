# TFTP Test Suite

Comprehensive automated tests for the Snow-Owl TFTP server, including integration tests and RFC 7440 windowsize tests.

## Quick Start

### Run All Tests (Recommended)

Run the complete test suite including integration and windowsize tests:

```bash
# Build and run all tests
cd crates/snow-owl-tftp
cargo build --release
./tests/run-all-tests.sh
```

### Option 1: Docker (Integration Tests Only)

Run tests in a consistent Linux environment using Docker:

```bash
cd crates/snow-owl-tftp/tests
./run-docker-tests.sh
```

This automatically builds the Docker image and runs all integration tests.

### Option 2: Individual Test Suites

Run specific test suites:

```bash
# Build the server first
cargo build --release

# Run integration tests only
./tests/integration-test.sh

# Run windowsize tests only (requires atftp)
./tests/windowsize-test.sh

# Run performance analysis (requires Python 3)
./tests/windowsize-analyzer.py performance
```

**Note:** macOS users may experience issues with the built-in TFTP client. Docker is recommended for consistent results.

## Test Suites

### 1. Integration Tests (`integration-test.sh`)

Core TFTP functionality and RFC compliance tests.

✅ **Basic Operations**
- Read requests (RRQ)
- Write requests (WRQ)
- Large file transfers
- NETASCII mode transfers

✅ **RFC Compliance**
- Path traversal protection
- Transfer size validation
- Timeout handling
- Option negotiation

✅ **Security Controls**
- Write pattern validation (allowed/denied)
- Access control enforcement
- Audit logging

✅ **Performance**
- Concurrent transfers
- Sequential transfers
- File integrity (MD5 checksums)

### 2. Windowsize Tests (`windowsize-test.sh`)

RFC 7440 windowsize option testing with 32 comprehensive test cases.

✅ **Windowsize Values**
- Tests 1-8: Small file (1KB) with windowsize 1-8
- Tests 9-16: Medium file (10KB) with windowsize 1, 2, 4, 8, 12, 16, 24, 32
- Tests 17-24: Large file (100KB) with windowsize 1, 2, 4, 8, 16, 32, 48, 64
- Tests 25-28: XLarge file (512KB) with windowsize 1, 8, 32, 64
- Tests 29-30: Single block edge cases
- Tests 31-32: Exact window boundary cases

✅ **Performance Metrics**
- Transfer time measurement
- Throughput calculation (Mbps)
- Packet and ACK counting
- Retransmission tracking
- File integrity verification

**See:** [WINDOWSIZE_TESTS.md](WINDOWSIZE_TESTS.md) for detailed documentation.

### 3. Performance Analyzer (`windowsize-analyzer.py`)

Advanced Python-based testing with detailed metrics:

```bash
# Quick test (windowsize 1-8)
./tests/windowsize-analyzer.py quick

# Full suite (all 32 tests)
./tests/windowsize-analyzer.py full

# Performance comparison
./tests/windowsize-analyzer.py performance
```

**Metrics provided:**
- Transfer time and throughput
- Total packets and ACKs
- Retransmission rate
- Packet loss rate
- Average RTT

## Test Output

The script provides colorized output:

- 🟢 **PASS** - Test succeeded
- 🔴 **FAIL** - Test failed (with error details)
- 🟡 **SKIP** - Test skipped (missing dependencies)

Example output:

```
================================================
  Snow-Owl TFTP Integration Tests
================================================

Setting up test environment...
Test environment ready

Starting TFTP server...
Server started (PID: 12345)

Running tests...

Running: Test 1: Basic RRQ... ✓ PASS - Test 1: Basic RRQ
Running: Test 2: Large file transfer... ✓ PASS - Test 2: Large file transfer
Running: Test 3: Basic WRQ... ✓ PASS - Test 3: Basic WRQ
...

================================================
  Test Summary
================================================
Total:   10
Passed:  10
Failed:  0
Skipped: 0

All tests passed!
```

## Requirements

### Integration Tests

- `tftp` client (tftp-hpa or compatible)
- `md5sum` or `md5` (for file integrity checks)

**Installation:**

```bash
# Ubuntu/Debian
sudo apt-get install tftp-hpa

# macOS
brew install tftp-hpa
# or use built-in tftp

# CentOS/RHEL
sudo yum install tftp
```

### Windowsize Tests

- `atftp` client (supports RFC 7440 windowsize option)
- `md5sum` or `md5` (for file integrity checks)

**Installation:**

```bash
# Ubuntu/Debian
sudo apt-get install atftp

# macOS
brew install atftp

# CentOS/RHEL
sudo yum install atftp
```

### Performance Analyzer

- Python 3.6+ (no external dependencies required)

**Check version:**

```bash
python3 --version
```

## Test Details

### Test 1: Basic RRQ

Validates basic read request functionality with a small text file.

### Test 2: Large File Transfer

Tests 1MB binary file transfer and validates integrity with MD5 checksums.

### Test 3: Basic WRQ

Tests file upload (write request) with content verification.

### Test 4: Write Pattern (Allowed)

Verifies that files matching allowed patterns (*.txt) can be uploaded.

### Test 5: Write Pattern (Denied)

Ensures files not matching allowed patterns (*.exe) are rejected.

### Test 6: Path Traversal Prevention

Validates security - attempts to access `../../etc/passwd` should fail.

### Test 7: Concurrent Transfers

Tests server handling of 3 simultaneous downloads.

### Test 8: Audit Logging

Verifies audit events are logged correctly.

### Test 9: NETASCII Mode

Tests NETASCII transfer mode with line ending conversion.

### Test 10: Sequential Transfers

Validates server handles multiple sequential requests without issues.

## Docker Testing

### Prerequisites

- Docker installed and running
- Docker daemon accessible

### Build and Run

The `run-docker-tests.sh` script handles everything automatically:

1. Builds a Docker image with Rust and TFTP client
2. Compiles the TFTP server in release mode
3. Runs all 10 integration tests
4. Removes the container after completion

### Manual Docker Commands

If you prefer manual control:

```bash
# Build the image
docker build -t snow-owl-tftp-test -f Dockerfile ../../..

# Run tests
docker run --rm snow-owl-tftp-test

# Run with interactive shell (for debugging)
docker run --rm -it snow-owl-tftp-test /bin/bash
```

## Troubleshooting

### Docker: Image build fails

- Ensure you're in the correct directory: `crates/snow-owl-tftp/tests`
- Check Docker has enough disk space: `docker system df`
- Try cleaning Docker cache: `docker system prune -a`

### Server fails to start

```bash
# Check if port is already in use
sudo netstat -tulpn | grep 6969

# Check server logs
cat /tmp/tftp-test-*/logs/server.log
```

### Tests fail with "tftp: command not found"

```bash
# Install TFTP client
sudo apt-get install tftp-hpa  # Ubuntu/Debian
brew install tftp-hpa          # macOS
```

### Permission errors

```bash
# Ensure test directory is writable
ls -la /tmp/

# Run with sudo if needed (for port 69)
sudo ./integration-test.sh
```

### Tests fail with "didn't receive answer from remote"

This error typically indicates IPv4/IPv6 binding issues:

```bash
# The test script uses IPv4 (0.0.0.0:6969 and 127.0.0.1)
# If you manually run the server, ensure it binds to IPv4:
bind_addr = "0.0.0.0:6969"  # IPv4
# NOT: bind_addr = "[::]:6969"  # IPv6 may not accept IPv4 clients on macOS
```

### Tests timeout

- Check firewall settings
- Verify server is listening: `sudo netstat -ulnp | grep 6969` (Linux) or `netstat -an | grep 6969` (macOS)
- Increase timeout in test script

## Advanced Usage

### Run specific test

Edit `integration-test.sh` and comment out tests you don't want to run:

```bash
# main() {
#     ...
#     # run_test "Test 1: Basic RRQ" test_basic_rrq
#     run_test "Test 2: Large file transfer" test_large_file
#     ...
# }
```

### Debug mode

Enable detailed logging:

```bash
# Edit tftp.toml in the test to use "debug" level
RUST_LOG=debug ./integration-test.sh
```

### Keep test environment

Comment out the cleanup trap to inspect files after tests:

```bash
# trap cleanup EXIT  # Comment this line
./integration-test.sh
```

Then inspect:

```bash
ls -la /tmp/tftp-test-*/
cat /tmp/tftp-test-*/logs/tftp.log
```

## Continuous Integration

See [integration-testing.md](../docs/integration-testing.md) for GitHub Actions workflow configuration.

## Related Documentation

- [Integration Testing Guide](../docs/integration-testing.md) - Comprehensive testing documentation
- [RFC Compliance Improvements](../docs/rfc-compliance-improvements.md) - Details on RFC fixes
- [Write Operations](../docs/write-operations.md) - Write operation documentation

## Contributing

When adding new features, please:

1. Add corresponding integration tests
2. Update this README with test descriptions
3. Ensure all tests pass before submitting PR

## License

Same as Snow-Owl project (MIT OR Apache-2.0)
