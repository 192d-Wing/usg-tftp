#!/bin/bash
# windowsize-test.sh - RFC 7440 windowsize option testing
# Tests windowsize values 1-32 with various file sizes

set -e  # Exit on error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
TEST_DIR="/tmp/tftp-windowsize-test-$$"
SERVER_PORT="6970"
SERVER_PID=""
RESULTS_FILE="$TEST_DIR/windowsize-results.txt"
PASSED=0
FAILED=0
SKIPPED=0

# Test file sizes
SMALL_SIZE=$((1024))           # 1 KB - fits in 2 blocks
MEDIUM_SIZE=$((10240))         # 10 KB - fits in 20 blocks
LARGE_SIZE=$((102400))         # 100 KB - 200 blocks
XLARGE_SIZE=$((524288))        # 512 KB - 1024 blocks

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
    echo -e "${BLUE}  Snow-Owl TFTP Windowsize Tests (RFC 7440)${NC}"
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
enabled = false

[multicast]
enabled = false

[rfc7440]
enabled = true
default_windowsize = 16
EOF

    # Create test files of various sizes
    dd if=/dev/urandom of="$TEST_DIR/root/small.bin" bs=1 count=$SMALL_SIZE 2>/dev/null
    dd if=/dev/urandom of="$TEST_DIR/root/medium.bin" bs=1 count=$MEDIUM_SIZE 2>/dev/null
    dd if=/dev/urandom of="$TEST_DIR/root/large.bin" bs=1 count=$LARGE_SIZE 2>/dev/null
    dd if=/dev/urandom of="$TEST_DIR/root/xlarge.bin" bs=1 count=$XLARGE_SIZE 2>/dev/null

    # Create edge case test files
    dd if=/dev/urandom of="$TEST_DIR/root/single-block.bin" bs=512 count=1 2>/dev/null
    dd if=/dev/urandom of="$TEST_DIR/root/exact-window.bin" bs=8192 count=1 2>/dev/null  # 16 blocks exactly

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

# Check if atftp is available (supports windowsize option)
check_requirements() {
    echo -e "${BLUE}Checking requirements...${NC}"

    if ! command -v atftp &> /dev/null; then
        echo -e "${RED}ERROR: atftp not found${NC}"
        echo -e "${YELLOW}Install with: sudo apt-get install atftp${NC}"
        exit 1
    fi

    if ! command -v md5sum &> /dev/null && ! command -v md5 &> /dev/null; then
        echo -e "${YELLOW}Warning: md5sum/md5 not found, file verification will be skipped${NC}"
    fi

    echo -e "${GREEN}All requirements met${NC}"
    echo ""
}

# Test windowsize with a specific value and file
test_windowsize() {
    local windowsize=$1
    local file=$2
    local test_num=$3

    cd "$TEST_DIR/client"

    local output_file="${file%.bin}-ws${windowsize}.bin"

    # Use atftp with windowsize option
    timeout 30 atftp --option "windowsize $windowsize" \
        --get -r "$file" -l "$output_file" 127.0.0.1 $SERVER_PORT 2>&1

    if [ ! -f "$output_file" ]; then
        echo "File not downloaded"
        return 1
    fi

    # Verify file integrity
    local orig_md5=$(calculate_md5 "$TEST_DIR/root/$file")
    local recv_md5=$(calculate_md5 "$output_file")

    if [ "$orig_md5" != "$recv_md5" ] && [ "$orig_md5" != "md5_unavailable" ]; then
        echo "MD5 mismatch: orig=$orig_md5, recv=$recv_md5"
        rm -f "$output_file"
        return 1
    fi

    rm -f "$output_file"
    return 0
}

# ============================================================================
# Windowsize Test Cases (1-32)
# ============================================================================

# Tests 1-8: Small file (1 KB) with windowsize 1-8
test_ws_01() { test_windowsize 1 "small.bin" 1; }
test_ws_02() { test_windowsize 2 "small.bin" 2; }
test_ws_03() { test_windowsize 3 "small.bin" 3; }
test_ws_04() { test_windowsize 4 "small.bin" 4; }
test_ws_05() { test_windowsize 5 "small.bin" 5; }
test_ws_06() { test_windowsize 6 "small.bin" 6; }
test_ws_07() { test_windowsize 7 "small.bin" 7; }
test_ws_08() { test_windowsize 8 "small.bin" 8; }

# Tests 9-16: Medium file (10 KB) with windowsize 1, 2, 4, 8, 12, 16, 24, 32
test_ws_09() { test_windowsize 1 "medium.bin" 9; }
test_ws_10() { test_windowsize 2 "medium.bin" 10; }
test_ws_11() { test_windowsize 4 "medium.bin" 11; }
test_ws_12() { test_windowsize 8 "medium.bin" 12; }
test_ws_13() { test_windowsize 12 "medium.bin" 13; }
test_ws_14() { test_windowsize 16 "medium.bin" 14; }
test_ws_15() { test_windowsize 24 "medium.bin" 15; }
test_ws_16() { test_windowsize 32 "medium.bin" 16; }

# Tests 17-24: Large file (100 KB) with windowsize 1, 2, 4, 8, 16, 32, 48, 64
test_ws_17() { test_windowsize 1 "large.bin" 17; }
test_ws_18() { test_windowsize 2 "large.bin" 18; }
test_ws_19() { test_windowsize 4 "large.bin" 19; }
test_ws_20() { test_windowsize 8 "large.bin" 20; }
test_ws_21() { test_windowsize 16 "large.bin" 21; }
test_ws_22() { test_windowsize 32 "large.bin" 22; }
test_ws_23() { test_windowsize 48 "large.bin" 23; }
test_ws_24() { test_windowsize 64 "large.bin" 24; }

# Tests 25-28: XLarge file (512 KB) with various windowsizes
test_ws_25() { test_windowsize 1 "xlarge.bin" 25; }
test_ws_26() { test_windowsize 8 "xlarge.bin" 26; }
test_ws_27() { test_windowsize 32 "xlarge.bin" 27; }
test_ws_28() { test_windowsize 64 "xlarge.bin" 28; }

# Tests 29-30: Edge cases - single block transfer
test_ws_29() { test_windowsize 1 "single-block.bin" 29; }
test_ws_30() { test_windowsize 16 "single-block.bin" 30; }

# Tests 31-32: Edge cases - exact window boundary
test_ws_31() { test_windowsize 16 "exact-window.bin" 31; }
test_ws_32() { test_windowsize 32 "exact-window.bin" 32; }

# ============================================================================
# Main Test Execution
# ============================================================================

run_test() {
    local test_name="$1"
    local test_num="$2"
    shift 2
    local test_func="$@"

    echo -n "Test $test_num: $test_name... "

    if eval "$test_func" >> "$RESULTS_FILE" 2>&1; then
        print_result "$test_name" "PASS"
    else
        local error=$(tail -1 "$RESULTS_FILE")
        print_result "$test_name" "FAIL" "$error"
    fi

    echo "" >> "$RESULTS_FILE"
}

main() {
    print_header

    check_requirements
    setup_test_env

    # Initialize results file
    echo "Snow-Owl TFTP Windowsize Test Results (RFC 7440)" > "$RESULTS_FILE"
    echo "================================================" >> "$RESULTS_FILE"
    echo "Date: $(date)" >> "$RESULTS_FILE"
    echo "" >> "$RESULTS_FILE"

    start_server

    echo -e "${BLUE}Running windowsize tests (1-32)...${NC}"
    echo ""

    # Run all 32 windowsize tests
    run_test "Windowsize 1 with small file (1KB)" 1 test_ws_01
    run_test "Windowsize 2 with small file (1KB)" 2 test_ws_02
    run_test "Windowsize 3 with small file (1KB)" 3 test_ws_03
    run_test "Windowsize 4 with small file (1KB)" 4 test_ws_04
    run_test "Windowsize 5 with small file (1KB)" 5 test_ws_05
    run_test "Windowsize 6 with small file (1KB)" 6 test_ws_06
    run_test "Windowsize 7 with small file (1KB)" 7 test_ws_07
    run_test "Windowsize 8 with small file (1KB)" 8 test_ws_08

    run_test "Windowsize 1 with medium file (10KB)" 9 test_ws_09
    run_test "Windowsize 2 with medium file (10KB)" 10 test_ws_10
    run_test "Windowsize 4 with medium file (10KB)" 11 test_ws_11
    run_test "Windowsize 8 with medium file (10KB)" 12 test_ws_12
    run_test "Windowsize 12 with medium file (10KB)" 13 test_ws_13
    run_test "Windowsize 16 with medium file (10KB)" 14 test_ws_14
    run_test "Windowsize 24 with medium file (10KB)" 15 test_ws_15
    run_test "Windowsize 32 with medium file (10KB)" 16 test_ws_16

    run_test "Windowsize 1 with large file (100KB)" 17 test_ws_17
    run_test "Windowsize 2 with large file (100KB)" 18 test_ws_18
    run_test "Windowsize 4 with large file (100KB)" 19 test_ws_19
    run_test "Windowsize 8 with large file (100KB)" 20 test_ws_20
    run_test "Windowsize 16 with large file (100KB)" 21 test_ws_21
    run_test "Windowsize 32 with large file (100KB)" 22 test_ws_22
    run_test "Windowsize 48 with large file (100KB)" 23 test_ws_23
    run_test "Windowsize 64 with large file (100KB)" 24 test_ws_24

    run_test "Windowsize 1 with xlarge file (512KB)" 25 test_ws_25
    run_test "Windowsize 8 with xlarge file (512KB)" 26 test_ws_26
    run_test "Windowsize 32 with xlarge file (512KB)" 27 test_ws_27
    run_test "Windowsize 64 with xlarge file (512KB)" 28 test_ws_28

    run_test "Windowsize 1 with single block file" 29 test_ws_29
    run_test "Windowsize 16 with single block file" 30 test_ws_30

    run_test "Windowsize 16 with exact window boundary" 31 test_ws_31
    run_test "Windowsize 32 with exact window boundary" 32 test_ws_32

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
