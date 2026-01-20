#!/bin/bash
# integration-test.sh - Automated TFTP integration tests
# Tests RFC compliance improvements and core functionality

set -e  # Exit on error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
TEST_DIR="/tmp/tftp-test-$$"
SERVER_PORT="6969"
SERVER_PORT_IPV6="6970"
SERVER_PID=""
SERVER_PID_IPV6=""
RESULTS_FILE="$TEST_DIR/test-results.txt"
PASSED=0
FAILED=0
SKIPPED=0

# Cleanup function
cleanup() {
    echo -e "\n${YELLOW}Cleaning up...${NC}"

    # Kill IPv4 server if running
    if [ -n "$SERVER_PID" ]; then
        kill $SERVER_PID 2>/dev/null || true
        wait $SERVER_PID 2>/dev/null || true
    fi

    # Kill IPv6 server if running
    if [ -n "$SERVER_PID_IPV6" ]; then
        kill $SERVER_PID_IPV6 2>/dev/null || true
        wait $SERVER_PID_IPV6 2>/dev/null || true
    fi

    # Remove test directory
    rm -rf "$TEST_DIR"

    echo -e "${GREEN}Cleanup complete${NC}"
}

trap cleanup EXIT

# Print header
print_header() {
    echo -e "${BLUE}================================================${NC}"
    echo -e "${BLUE}  Snow-Owl TFTP Integration Tests${NC}"
    echo -e "${BLUE}================================================${NC}"
    echo ""
}

# Print test result
print_result() {
    local test_name="$1"
    local result="$2"
    local details="$3"

    if [ "$result" = "PASS" ]; then
        echo -e "${GREEN}✓ PASS${NC} - $test_name"
        ((PASSED++)) || true
    elif [ "$result" = "FAIL" ]; then
        echo -e "${RED}✗ FAIL${NC} - $test_name"
        if [ -n "$details" ]; then
            echo -e "  ${RED}Error: $details${NC}"
        fi
        ((FAILED++)) || true
    elif [ "$result" = "SKIP" ]; then
        echo -e "${YELLOW}○ SKIP${NC} - $test_name"
        if [ -n "$details" ]; then
            echo -e "  ${YELLOW}Reason: $details${NC}"
        fi
        ((SKIPPED++)) || true
    fi

    echo "$result - $test_name" >> "$RESULTS_FILE"
    if [ -n "$details" ]; then
        echo "  $details" >> "$RESULTS_FILE"
    fi
}

# Run a test
run_test() {
    local test_name="$1"
    shift
    local test_cmd="$@"

    echo -n "Running: $test_name... "

    local result
    eval "$test_cmd" >> "$RESULTS_FILE" 2>&1
    result=$?

    if [ $result -eq 0 ]; then
        print_result "$test_name" "PASS"
    elif [ $result -eq 2 ]; then
        local reason=$(tail -1 "$RESULTS_FILE")
        print_result "$test_name" "SKIP" "$reason"
    else
        local error=$(tail -1 "$RESULTS_FILE")
        print_result "$test_name" "FAIL" "$error"
    fi

    echo "" >> "$RESULTS_FILE"
}

# Setup test environment
setup_test_env() {
    echo -e "${BLUE}Setting up test environment...${NC}"

    # Create directories
    mkdir -p "$TEST_DIR"/{root,client,logs}

    # Create test config
    cat > "$TEST_DIR/tftp.toml" <<EOF
root_dir = "$TEST_DIR/root"
bind_addr = "0.0.0.0:$SERVER_PORT"
max_file_size_bytes = 10485760

[logging]
level = "info"
format = "text"
file = "$TEST_DIR/logs/tftp.log"
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
EOF

    # Create IPv6 test config (dual-stack server)
    cat > "$TEST_DIR/tftp-ipv6.toml" <<EOF
root_dir = "$TEST_DIR/root"
bind_addr = "[::]:$SERVER_PORT_IPV6"
max_file_size_bytes = 10485760

[logging]
level = "info"
format = "text"
file = "$TEST_DIR/logs/tftp-ipv6.log"
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
EOF

    # Create test files
    echo "Hello, TFTP!" > "$TEST_DIR/root/hello.txt"
    dd if=/dev/urandom of="$TEST_DIR/root/random.bin" bs=1024 count=100 2>/dev/null
    printf "Line 1\nLine 2\nLine 3\n" > "$TEST_DIR/root/lines.txt"
    dd if=/dev/zero of="$TEST_DIR/root/large.bin" bs=1024 count=1024 2>/dev/null

    # Create Unix line ending file for NETASCII testing
    printf "Unix\nLine\nEndings\n" > "$TEST_DIR/root/unix-lines.txt"

    echo -e "${GREEN}Test environment ready${NC}"
    echo ""
}

# Start TFTP server
start_server() {
    echo -e "${BLUE}Starting TFTP server...${NC}"

    # Find the server binary (check multiple possible locations)
    SERVER_BIN=""

    # Try project root paths first
    if [ -f "../../../target/release/snow-owl-tftp-server" ]; then
        SERVER_BIN="../../../target/release/snow-owl-tftp-server"
    elif [ -f "../../../target/debug/snow-owl-tftp-server" ]; then
        SERVER_BIN="../../../target/debug/snow-owl-tftp-server"
    # Try from crate directory
    elif [ -f "../../target/release/snow-owl-tftp-server" ]; then
        SERVER_BIN="../../target/release/snow-owl-tftp-server"
    elif [ -f "../../target/debug/snow-owl-tftp-server" ]; then
        SERVER_BIN="../../target/debug/snow-owl-tftp-server"
    else
        echo -e "${RED}ERROR: Server binary not found.${NC}"
        echo -e "${RED}Please run 'cargo build --release' from the project root first.${NC}"
        exit 1
    fi

    # Start server in background
    "$SERVER_BIN" --config "$TEST_DIR/tftp.toml" > "$TEST_DIR/logs/server.log" 2>&1 &
    SERVER_PID=$!

    # Wait for server to start
    sleep 2

    # Check if server is running
    if ! kill -0 $SERVER_PID 2>/dev/null; then
        echo -e "${RED}ERROR: Server failed to start${NC}"
        cat "$TEST_DIR/logs/server.log"
        exit 1
    fi

    echo -e "${GREEN}Server started (PID: $SERVER_PID)${NC}"
    echo ""
}

# Start IPv6 TFTP server (for IPv6 tests)
start_server_ipv6() {
    # Check if IPv6 is available
    if ! ip -6 addr show lo 2>/dev/null | grep -q "inet6 ::1"; then
        echo -e "${YELLOW}IPv6 not available, skipping IPv6 server${NC}"
        return 1
    fi

    echo -e "${BLUE}Starting IPv6 TFTP server...${NC}"

    # Find the server binary (reuse logic from start_server)
    SERVER_BIN=""
    if [ -f "../../../target/release/snow-owl-tftp-server" ]; then
        SERVER_BIN="../../../target/release/snow-owl-tftp-server"
    elif [ -f "../../../target/debug/snow-owl-tftp-server" ]; then
        SERVER_BIN="../../../target/debug/snow-owl-tftp-server"
    elif [ -f "../../target/release/snow-owl-tftp-server" ]; then
        SERVER_BIN="../../target/release/snow-owl-tftp-server"
    elif [ -f "../../target/debug/snow-owl-tftp-server" ]; then
        SERVER_BIN="../../target/debug/snow-owl-tftp-server"
    else
        echo -e "${YELLOW}Server binary not found for IPv6${NC}"
        return 1
    fi

    # Start IPv6 server in background
    "$SERVER_BIN" --config "$TEST_DIR/tftp-ipv6.toml" > "$TEST_DIR/logs/server-ipv6.log" 2>&1 &
    SERVER_PID_IPV6=$!

    # Wait for server to start
    sleep 2

    # Check if server is running
    if ! kill -0 $SERVER_PID_IPV6 2>/dev/null; then
        echo -e "${YELLOW}IPv6 server failed to start (IPv6 tests will be skipped)${NC}"
        cat "$TEST_DIR/logs/server-ipv6.log"
        SERVER_PID_IPV6=""
        return 1
    fi

    echo -e "${GREEN}IPv6 Server started (PID: $SERVER_PID_IPV6)${NC}"
    echo ""
    return 0
}

# Check if required tools are available
check_requirements() {
    echo -e "${BLUE}Checking requirements...${NC}"

    local missing=""

    if ! command -v tftp &> /dev/null; then
        missing="$missing tftp"
    fi

    if ! command -v md5sum &> /dev/null && ! command -v md5 &> /dev/null; then
        missing="$missing md5sum"
    fi

    if [ -n "$missing" ]; then
        echo -e "${YELLOW}Warning: Missing optional tools:$missing${NC}"
        echo -e "${YELLOW}Some tests may be skipped${NC}"
    else
        echo -e "${GREEN}All requirements met${NC}"
    fi

    echo ""
}

# Calculate MD5 checksum (cross-platform)
calculate_md5() {
    local file="$1"
    if command -v md5sum &> /dev/null; then
        md5sum "$file" | awk '{print $1}'
    elif command -v md5 &> /dev/null; then
        md5 -q "$file"
    else
        echo "md5_unavailable"
    fi
}

# ============================================================================
# Test Cases
# ============================================================================

# Test 1: Basic RRQ (Read Request)
test_basic_rrq() {
    cd "$TEST_DIR/client"

    tftp 127.0.0.1 $SERVER_PORT <<EOF
mode octet
get hello.txt
quit
EOF

    if [ ! -f hello.txt ]; then
        echo "File not downloaded"
        return 1
    fi

    if ! grep -q "Hello, TFTP!" hello.txt; then
        echo "File content incorrect"
        return 1
    fi

    rm -f hello.txt
    return 0
}

# Test 2: Large file transfer
test_large_file() {
    cd "$TEST_DIR/client"

    tftp 127.0.0.1 $SERVER_PORT <<EOF
mode octet
get large.bin
quit
EOF

    if [ ! -f large.bin ]; then
        echo "Large file not downloaded"
        return 1
    fi

    local orig_md5=$(calculate_md5 "$TEST_DIR/root/large.bin")
    local recv_md5=$(calculate_md5 "large.bin")

    if [ "$orig_md5" != "$recv_md5" ]; then
        echo "MD5 mismatch: orig=$orig_md5, recv=$recv_md5"
        return 1
    fi

    rm -f large.bin
    return 0
}

# Test 3: Basic WRQ (Write Request)
test_basic_wrq() {
    cd "$TEST_DIR/client"

    echo "Upload test content" > upload.txt

    tftp 127.0.0.1 $SERVER_PORT <<EOF
mode octet
put upload.txt
quit
EOF

    # Wait for server to complete async file write operations
    sleep 1

    if [ ! -f "$TEST_DIR/root/upload.txt" ]; then
        echo "File not uploaded to server"
        return 1
    fi

    if ! grep -q "Upload test content" "$TEST_DIR/root/upload.txt"; then
        echo "Uploaded file content incorrect"
        return 1
    fi

    rm -f upload.txt "$TEST_DIR/root/upload.txt"
    return 0
}

# Test 4: Write pattern validation (allowed)
test_write_pattern_allowed() {
    cd "$TEST_DIR/client"

    echo "Allowed content" > allowed.txt

    tftp 127.0.0.1 $SERVER_PORT <<EOF
mode octet
put allowed.txt
quit
EOF

    # Wait for server to complete async file write operations
    sleep 1

    if [ ! -f "$TEST_DIR/root/allowed.txt" ]; then
        echo "Allowed file was not uploaded"
        return 1
    fi

    rm -f allowed.txt "$TEST_DIR/root/allowed.txt"
    return 0
}

# Test 5: Write pattern validation (denied)
test_write_pattern_denied() {
    cd "$TEST_DIR/client"

    echo "Blocked content" > blocked.exe

    # This should fail
    if tftp 127.0.0.1 $SERVER_PORT <<EOF 2>&1 | grep -iq "error"
mode octet
put blocked.exe
quit
EOF
    then
        rm -f blocked.exe
        return 0
    else
        echo "Blocked file was incorrectly uploaded"
        rm -f blocked.exe "$TEST_DIR/root/blocked.exe"
        return 1
    fi
}

# Test 6: Path traversal prevention
test_path_traversal() {
    cd "$TEST_DIR/client"

    # This should fail
    if tftp 127.0.0.1 $SERVER_PORT <<EOF 2>&1 | grep -iq "error\|denied"
mode octet
get ../../etc/passwd
quit
EOF
    then
        return 0
    else
        echo "Path traversal was not blocked"
        return 1
    fi
}

# Test 7: Concurrent transfers
test_concurrent_transfers() {
    cd "$TEST_DIR/client"

    # Launch 3 concurrent downloads
    for i in 1 2 3; do
        (
            tftp 127.0.0.1 $SERVER_PORT <<EOF
mode octet
get random.bin random-$i.bin
quit
EOF
        ) &
    done

    # Wait 5 seconds for all transfers
    sleep 5
    # Verify all files downloaded correctly
    local orig_md5=$(calculate_md5 "$TEST_DIR/root/random.bin")

    for i in 1 2 3; do
        if [ ! -f "random-$i.bin" ]; then
            echo "Concurrent download $i failed"
            return 1
        fi

        local recv_md5=$(calculate_md5 "random-$i.bin")
        if [ "$orig_md5" != "$recv_md5" ]; then
            echo "Concurrent download $i corrupted"
            return 1
        fi
    done

    rm -f random-*.bin
    return 0
}

# Test 8: Audit logging verification
test_audit_logging() {
    local log_file="$TEST_DIR/logs/tftp.log"

    if [ ! -f "$log_file" ]; then
        echo "Log file not found"
        return 1
    fi

    # Check for key audit events
    if ! grep -q "read_request" "$log_file" 2>/dev/null; then
        echo "No read_request events in audit log"
        return 1
    fi

    if ! grep -q "transfer_completed" "$log_file" 2>/dev/null; then
        echo "No transfer_completed events in audit log"
        return 1
    fi

    return 0
}

# Test 9: NETASCII mode
test_netascii_mode() {
    cd "$TEST_DIR/client"

    tftp 127.0.0.1 $SERVER_PORT <<EOF
mode netascii
get unix-lines.txt
quit
EOF

    if [ ! -f unix-lines.txt ]; then
        echo "NETASCII file not downloaded"
        return 1
    fi

    # Just verify it downloaded
    rm -f unix-lines.txt
    return 0
}

# Test 10: Server handles multiple sequential transfers
test_sequential_transfers() {
    cd "$TEST_DIR/client"

    for i in 1 2 3 4 5; do
        tftp 127.0.0.1 $SERVER_PORT <<EOF
mode octet
get hello.txt hello-$i.txt
quit
EOF

        if [ ! -f "hello-$i.txt" ]; then
            echo "Sequential transfer $i failed"
            return 1
        fi
    done

    rm -f hello-*.txt
    return 0
}

# ============================================================================
# IPv6 Tests (Tests 11-14)
# ============================================================================

# Check if IPv6 is available on the system
check_ipv6_available() {
    if ip -6 addr show lo 2>/dev/null | grep -q "inet6 ::1"; then
        return 0
    fi
    return 1
}

# Test 11: IPv6 Basic RRQ
test_ipv6_basic_rrq() {
    if ! check_ipv6_available; then
        echo "IPv6 not available on system"
        return 2  # Skip
    fi

    if ! command -v atftp &> /dev/null; then
        echo "atftp not available for IPv6 testing"
        return 2  # Skip
    fi

    # Check if IPv6 server is still running
    if [ -z "$SERVER_PID_IPV6" ] || ! kill -0 $SERVER_PID_IPV6 2>/dev/null; then
        echo "IPv6 server not running"
        return 2  # Skip
    fi

    cd "$TEST_DIR/client"

    # Use atftp for IPv6 (standard tftp client may not support IPv6)
    timeout 10 atftp -g -r hello.txt -l hello-ipv6.txt ::1 $SERVER_PORT_IPV6 2>&1 || true

    if [ ! -f hello-ipv6.txt ]; then
        echo "IPv6 file not downloaded"
        return 1
    fi

    if ! grep -q "Hello, TFTP!" hello-ipv6.txt; then
        echo "IPv6 file content incorrect"
        return 1
    fi

    rm -f hello-ipv6.txt
    return 0
}

# Test 12: IPv6 Large file transfer
test_ipv6_large_file() {
    if ! check_ipv6_available; then
        echo "IPv6 not available on system"
        return 2  # Skip
    fi

    if ! command -v atftp &> /dev/null; then
        echo "atftp not available for IPv6 testing"
        return 2  # Skip
    fi

    # Check if IPv6 server is still running
    if [ -z "$SERVER_PID_IPV6" ] || ! kill -0 $SERVER_PID_IPV6 2>/dev/null; then
        echo "IPv6 server not running"
        return 2  # Skip
    fi

    cd "$TEST_DIR/client"

    local orig_md5=$(calculate_md5 "$TEST_DIR/root/random.bin")

    # Use verbose mode and explicit options for debugging
    timeout 60 atftp -g -r random.bin -l random-ipv6.bin ::1 $SERVER_PORT_IPV6 2>&1 || true

    if [ ! -f random-ipv6.bin ]; then
        echo "IPv6 large file not downloaded"
        return 1
    fi

    local recv_md5=$(calculate_md5 "random-ipv6.bin")
    if [ "$orig_md5" != "$recv_md5" ]; then
        echo "IPv6 file checksum mismatch (orig: $orig_md5, recv: $recv_md5)"
        return 1
    fi

    rm -f random-ipv6.bin
    return 0
}

# Test 13: IPv6 Write Request
test_ipv6_basic_wrq() {
    if ! check_ipv6_available; then
        echo "IPv6 not available on system"
        return 2  # Skip
    fi

    if ! command -v atftp &> /dev/null; then
        echo "atftp not available for IPv6 testing"
        return 2  # Skip
    fi

    # Check if IPv6 server is still running
    if [ -z "$SERVER_PID_IPV6" ] || ! kill -0 $SERVER_PID_IPV6 2>/dev/null; then
        echo "IPv6 server not running"
        return 2  # Skip
    fi

    cd "$TEST_DIR/client"

    # Create file to upload
    echo "IPv6 upload test content" > upload-ipv6.txt

    # Run atftp put - don't capture output to avoid subshell hanging
    timeout 10 atftp -p -l upload-ipv6.txt -r upload-ipv6.txt ::1 $SERVER_PORT_IPV6 >/dev/null 2>&1 || true

    # Give server time to write file
    sleep 1

    if [ ! -f "$TEST_DIR/root/upload-ipv6.txt" ]; then
        echo "IPv6 file not uploaded"
        rm -f upload-ipv6.txt
        return 1
    fi

    if ! grep -q "IPv6 upload test content" "$TEST_DIR/root/upload-ipv6.txt"; then
        echo "IPv6 uploaded file content incorrect"
        rm -f upload-ipv6.txt
        return 1
    fi

    rm -f upload-ipv6.txt
    return 0
}

# Test 14: IPv6 Dual-stack (server on [::] accepts IPv4 clients)
test_ipv6_dual_stack() {
    if ! check_ipv6_available; then
        echo "IPv6 not available on system"
        return 2  # Skip
    fi

    # Check if IPv6 server is still running
    if [ -z "$SERVER_PID_IPV6" ] || ! kill -0 $SERVER_PID_IPV6 2>/dev/null; then
        echo "IPv6 server not running"
        return 2  # Skip
    fi

    cd "$TEST_DIR/client"

    # Test IPv4 client connecting to IPv6 dual-stack server
    tftp 127.0.0.1 $SERVER_PORT_IPV6 <<EOF
mode octet
get hello.txt hello-dualstack.txt
quit
EOF

    if [ ! -f hello-dualstack.txt ]; then
        echo "Dual-stack IPv4 connection failed"
        return 1
    fi

    if ! grep -q "Hello, TFTP!" hello-dualstack.txt; then
        echo "Dual-stack file content incorrect"
        return 1
    fi

    rm -f hello-dualstack.txt
    return 0
}

# Test 15: IPv6 Concurrent transfers
test_ipv6_concurrent_transfers() {
    if ! check_ipv6_available; then
        echo "IPv6 not available on system"
        return 2  # Skip
    fi

    if ! command -v atftp &> /dev/null; then
        echo "atftp not available for IPv6 testing"
        return 2  # Skip
    fi

    # Check if IPv6 server is still running
    if [ -z "$SERVER_PID_IPV6" ] || ! kill -0 $SERVER_PID_IPV6 2>/dev/null; then
        echo "IPv6 server not running"
        return 2  # Skip
    fi

    cd "$TEST_DIR/client"

    local orig_md5=$(calculate_md5 "$TEST_DIR/root/random.bin")

    # Launch 3 concurrent IPv6 downloads and collect PIDs
    local pids=""
    for i in 1 2 3; do
        timeout 30 atftp -g -r random.bin -l random-ipv6-$i.bin ::1 $SERVER_PORT_IPV6 >/dev/null 2>&1 &
        pids="$pids $!"
    done

    # Wait for specific PIDs only
    for pid in $pids; do
        wait $pid 2>/dev/null || true
    done

    # Verify all files
    for i in 1 2 3; do
        if [ ! -f "random-ipv6-$i.bin" ]; then
            echo "IPv6 concurrent download $i failed"
            rm -f random-ipv6-*.bin
            return 1
        fi

        local recv_md5=$(calculate_md5 "random-ipv6-$i.bin")
        if [ "$orig_md5" != "$recv_md5" ]; then
            echo "IPv6 concurrent download $i corrupted"
            rm -f random-ipv6-*.bin
            return 1
        fi
    done

    rm -f random-ipv6-*.bin
    return 0
}

# Test 16: IPv6 Sequential transfers
test_ipv6_sequential_transfers() {
    if ! check_ipv6_available; then
        echo "IPv6 not available on system"
        return 2  # Skip
    fi

    if ! command -v atftp &> /dev/null; then
        echo "atftp not available for IPv6 testing"
        return 2  # Skip
    fi

    # Check if IPv6 server is still running
    if [ -z "$SERVER_PID_IPV6" ] || ! kill -0 $SERVER_PID_IPV6 2>/dev/null; then
        echo "IPv6 server not running"
        return 2  # Skip
    fi

    cd "$TEST_DIR/client"

    # Perform 5 sequential IPv6 transfers
    for i in 1 2 3 4 5; do
        timeout 10 atftp -g -r hello.txt -l hello-ipv6-seq-$i.txt ::1 $SERVER_PORT_IPV6 >/dev/null 2>&1 || true

        if [ ! -f "hello-ipv6-seq-$i.txt" ]; then
            echo "IPv6 sequential transfer $i failed"
            rm -f hello-ipv6-seq-*.txt
            return 1
        fi

        if ! grep -q "Hello, TFTP!" "hello-ipv6-seq-$i.txt"; then
            echo "IPv6 sequential transfer $i content incorrect"
            rm -f hello-ipv6-seq-*.txt
            return 1
        fi
    done

    rm -f hello-ipv6-seq-*.txt
    return 0
}

# ============================================================================
# Main Test Execution
# ============================================================================

main() {
    print_header

    check_requirements
    setup_test_env

    # Initialize results file (after TEST_DIR is created)
    echo "Snow-Owl TFTP Integration Test Results" > "$RESULTS_FILE"
    echo "=======================================" >> "$RESULTS_FILE"
    echo "Date: $(date)" >> "$RESULTS_FILE"
    echo "" >> "$RESULTS_FILE"

    start_server

    # Try to start IPv6 server (may fail if IPv6 not available)
    IPV6_AVAILABLE=false
    if start_server_ipv6; then
        IPV6_AVAILABLE=true
    fi

    echo -e "${BLUE}Running tests...${NC}"
    echo ""

    # Run IPv4 tests (1-10)
    run_test "Test 1: Basic RRQ" test_basic_rrq
    run_test "Test 2: Large file transfer" test_large_file
    run_test "Test 3: Basic WRQ" test_basic_wrq
    run_test "Test 4: Write pattern (allowed)" test_write_pattern_allowed
    run_test "Test 5: Write pattern (denied)" test_write_pattern_denied
    run_test "Test 6: Path traversal prevention" test_path_traversal
    run_test "Test 7: Concurrent transfers" test_concurrent_transfers
    run_test "Test 8: Audit logging" test_audit_logging
    run_test "Test 9: NETASCII mode" test_netascii_mode
    run_test "Test 10: Sequential transfers" test_sequential_transfers

    # Run IPv6 tests (11-16) if IPv6 server is running
    if [ "$IPV6_AVAILABLE" = true ]; then
        echo ""
        echo -e "${BLUE}Running IPv6 tests...${NC}"
        echo ""
        run_test "Test 11: IPv6 Basic RRQ" test_ipv6_basic_rrq
        run_test "Test 12: IPv6 Large file transfer" test_ipv6_large_file
        run_test "Test 13: IPv6 Basic WRQ" test_ipv6_basic_wrq
        run_test "Test 14: IPv6 Dual-stack" test_ipv6_dual_stack
        run_test "Test 15: IPv6 Concurrent transfers" test_ipv6_concurrent_transfers
        run_test "Test 16: IPv6 Sequential transfers" test_ipv6_sequential_transfers
    else
        echo ""
        echo -e "${YELLOW}Skipping IPv6 tests (IPv6 not available)${NC}"
        echo ""
        ((SKIPPED+=6)) || true
    fi

    # Print summary
    echo ""
    echo -e "${BLUE}================================================${NC}"
    echo -e "${BLUE}  Test Summary${NC}"
    echo -e "${BLUE}================================================${NC}"
    echo -e "Total:   $((PASSED + FAILED + SKIPPED))"
    echo -e "${GREEN}Passed:  $PASSED${NC}"
    echo -e "${RED}Failed:  $FAILED${NC}"
    echo -e "${YELLOW}Skipped: $SKIPPED${NC}"
    echo ""

    # Save summary to results file
    echo "" >> "$RESULTS_FILE"
    echo "Summary:" >> "$RESULTS_FILE"
    echo "  Total:   $((PASSED + FAILED + SKIPPED))" >> "$RESULTS_FILE"
    echo "  Passed:  $PASSED" >> "$RESULTS_FILE"
    echo "  Failed:  $FAILED" >> "$RESULTS_FILE"
    echo "  Skipped: $SKIPPED" >> "$RESULTS_FILE"

    echo -e "${BLUE}Results saved to: $RESULTS_FILE${NC}"

    # Exit with appropriate code
    if [ $FAILED -gt 0 ]; then
        echo -e "\n${RED}Some tests failed!${NC}"
        exit 1
    else
        echo -e "\n${GREEN}All tests passed!${NC}"
        exit 0
    fi
}

# Run main function
main "$@"
