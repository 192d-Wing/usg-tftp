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
SERVER_PID=""
RESULTS_FILE="$TEST_DIR/test-results.txt"
PASSED=0
FAILED=0
SKIPPED=0

# Cleanup function
cleanup() {
    echo -e "\n${YELLOW}Cleaning up...${NC}"

    # Kill server if running
    if [ -n "$SERVER_PID" ]; then
        kill $SERVER_PID 2>/dev/null || true
        wait $SERVER_PID 2>/dev/null || true
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

    if eval "$test_cmd" >> "$RESULTS_FILE" 2>&1; then
        print_result "$test_name" "PASS"
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

    # Find the server binary
    SERVER_BIN=""
    if [ -f "../../../target/release/snow-owl-tftp" ]; then
        SERVER_BIN="../../../target/release/snow-owl-tftp"
    elif [ -f "../../../target/debug/snow-owl-tftp" ]; then
        SERVER_BIN="../../target/debug/snow-owl-tftp"
    else
        echo -e "${RED}ERROR: Server binary not found. Please run 'cargo build' first.${NC}"
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

    echo -e "${BLUE}Running tests...${NC}"
    echo ""

    # Run all tests
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
