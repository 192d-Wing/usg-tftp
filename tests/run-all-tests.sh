#!/bin/bash
# run-all-tests.sh - Master test runner for all TFTP tests
# Runs integration tests, windowsize tests, and optional performance tests

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

# Test results
TOTAL_PASSED=0
TOTAL_FAILED=0
TOTAL_SKIPPED=0

# Print header
print_header() {
    echo -e "${CYAN}╔════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║                                                ║${NC}"
    echo -e "${CYAN}║      Snow-Owl TFTP Test Suite Runner          ║${NC}"
    echo -e "${CYAN}║                                                ║${NC}"
    echo -e "${CYAN}╔════════════════════════════════════════════════╗${NC}"
    echo ""
}

# Print section header
print_section() {
    local title="$1"
    echo ""
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${BLUE}  $title${NC}"
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
}

# Check if a command exists
command_exists() {
    command -v "$1" &> /dev/null
}

# Parse test results from a results file
parse_results() {
    local results_file="$1"
    local test_name="$2"

    if [ ! -f "$results_file" ]; then
        echo -e "${RED}✗ Results file not found: $results_file${NC}"
        return 1
    fi

    local passed=$(grep -c "^PASS" "$results_file" || true)
    local failed=$(grep -c "^FAIL" "$results_file" || true)
    local skipped=$(grep -c "^SKIP" "$results_file" || true)

    TOTAL_PASSED=$((TOTAL_PASSED + passed))
    TOTAL_FAILED=$((TOTAL_FAILED + failed))
    TOTAL_SKIPPED=$((TOTAL_SKIPPED + skipped))

    echo -e "  ${test_name}: ${GREEN}${passed} passed${NC}, ${RED}${failed} failed${NC}, ${YELLOW}${skipped} skipped${NC}"
}

# Run a test suite
run_test_suite() {
    local test_script="$1"
    local test_name="$2"
    local results_file="$3"

    print_section "$test_name"

    if [ ! -f "$test_script" ]; then
        echo -e "${RED}✗ Test script not found: $test_script${NC}"
        TOTAL_FAILED=$((TOTAL_FAILED + 1))
        return 1
    fi

    if ! chmod +x "$test_script"; then
        echo -e "${RED}✗ Failed to make script executable${NC}"
        TOTAL_FAILED=$((TOTAL_FAILED + 1))
        return 1
    fi

    echo -e "${BLUE}Running $test_name...${NC}"
    echo ""

    # Capture test output to parse counts
    local test_output=$(mktemp)
    if "$test_script" > "$test_output" 2>&1; then
        cat "$test_output"
        echo ""
        echo -e "${GREEN}✓ $test_name completed successfully${NC}"

        # Parse test counts from output (strip ANSI color codes first)
        local passed=$(sed 's/\x1b\[[0-9;]*m//g' "$test_output" | grep -E "Passed:" | awk '{print $NF}' || echo "0")
        local failed=$(sed 's/\x1b\[[0-9;]*m//g' "$test_output" | grep -E "Failed:" | awk '{print $NF}' || echo "0")
        local skipped=$(sed 's/\x1b\[[0-9;]*m//g' "$test_output" | grep -E "Skipped:" | awk '{print $NF}' || echo "0")

        TOTAL_PASSED=$((TOTAL_PASSED + passed))
        TOTAL_FAILED=$((TOTAL_FAILED + failed))
        TOTAL_SKIPPED=$((TOTAL_SKIPPED + skipped))

        rm -f "$test_output"
        return 0
    else
        cat "$test_output"
        echo ""
        echo -e "${RED}✗ $test_name failed${NC}"

        # Try to parse counts even on failure (strip ANSI color codes first)
        local passed=$(sed 's/\x1b\[[0-9;]*m//g' "$test_output" | grep -E "Passed:" | awk '{print $NF}' || echo "0")
        local failed=$(sed 's/\x1b\[[0-9;]*m//g' "$test_output" | grep -E "Failed:" | awk '{print $NF}' || echo "0")
        local skipped=$(sed 's/\x1b\[[0-9;]*m//g' "$test_output" | grep -E "Skipped:" | awk '{print $NF}' || echo "0")

        TOTAL_PASSED=$((TOTAL_PASSED + passed))
        TOTAL_FAILED=$((TOTAL_FAILED + failed + 1))  # +1 for the suite failure
        TOTAL_SKIPPED=$((TOTAL_SKIPPED + skipped))

        rm -f "$test_output"
        return 1
    fi
}

# Check prerequisites
check_prerequisites() {
    print_section "Checking Prerequisites"

    local missing=""
    local warnings=""

    # Setup Rust environment
    export PATH="$HOME/.cargo/bin:$PATH"

    # Check for cargo
    if ! command_exists cargo; then
        missing="$missing cargo"
    else
        echo -e "${GREEN}✓ cargo found${NC}"
    fi

    # Check for tftp client
    if ! command_exists tftp; then
        warnings="$warnings tftp"
        echo -e "${YELLOW}⚠ tftp not found - some tests may be skipped${NC}"
    else
        echo -e "${GREEN}✓ tftp found${NC}"
    fi

    # Check for atftp (required for windowsize tests)
    if ! command_exists atftp; then
        warnings="$warnings atftp"
        echo -e "${YELLOW}⚠ atftp not found - windowsize tests will be skipped${NC}"
        echo -e "${YELLOW}  Install with: sudo apt-get install atftp${NC}"
    else
        echo -e "${GREEN}✓ atftp found (required for windowsize tests)${NC}"
    fi

    # Check for md5sum
    if ! command_exists md5sum && ! command_exists md5; then
        warnings="$warnings md5sum"
        echo -e "${YELLOW}⚠ md5sum/md5 not found - integrity checks may be skipped${NC}"
    else
        echo -e "${GREEN}✓ md5sum/md5 found${NC}"
    fi

    # Check for python3 (optional for advanced analysis)
    if command_exists python3; then
        echo -e "${GREEN}✓ python3 found (optional)${NC}"
    fi

    if [ -n "$missing" ]; then
        echo ""
        echo -e "${RED}✗ Missing required tools:$missing${NC}"
        return 1
    fi

    echo ""
    return 0
}

# Build the project
build_project() {
    print_section "Building Project"

    echo -e "${BLUE}Building TFTP server and client...${NC}"
    echo ""

    # Setup Rust environment
    export PATH="$HOME/.cargo/bin:$PATH"

    # Navigate to project root (from tests directory)
    cd ../../..

    # Build release binaries (TFTP package only to avoid SFTP dependency issues)
    if cargo build --release -p snow-owl-tftp 2>&1; then
        BUILD_EXIT=$?
    else
        BUILD_EXIT=$?
    fi

    # Return to tests directory
    cd crates/snow-owl-tftp/tests

    if [ $BUILD_EXIT -ne 0 ]; then
        echo ""
        echo -e "${RED}✗ Build failed${NC}"
        return 1
    fi

    echo ""
    echo -e "${GREEN}✓ Build successful${NC}"
    return 0
}

# Print usage
usage() {
    echo "Usage: $0 [options]"
    echo ""
    echo "Options:"
    echo "  -h, --help              Show this help message"
    echo "  -i, --integration       Run only integration tests"
    echo "  -w, --windowsize        Run only windowsize tests"
    echo "  -p, --performance       Include performance analysis (Python)"
    echo "  -s, --skip-build        Skip building the project"
    echo "  -a, --all               Run all tests (default)"
    echo ""
    echo "Examples:"
    echo "  $0                      # Run all tests"
    echo "  $0 --integration        # Run only integration tests"
    echo "  $0 --windowsize         # Run only windowsize tests"
    echo "  $0 -p                   # Run all tests with performance analysis"
    echo ""
}

# Main function
main() {
    local run_integration=false
    local run_windowsize=false
    local run_performance=false
    local skip_build=false
    local run_all=true

    # Parse command line arguments
    while [[ $# -gt 0 ]]; do
        case $1 in
            -h|--help)
                usage
                exit 0
                ;;
            -i|--integration)
                run_integration=true
                run_all=false
                shift
                ;;
            -w|--windowsize)
                run_windowsize=true
                run_all=false
                shift
                ;;
            -p|--performance)
                run_performance=true
                shift
                ;;
            -s|--skip-build)
                skip_build=true
                shift
                ;;
            -a|--all)
                run_all=true
                shift
                ;;
            *)
                echo -e "${RED}Unknown option: $1${NC}"
                usage
                exit 1
                ;;
        esac
    done

    # Default to all tests if none specified
    if [ "$run_all" = true ]; then
        run_integration=true
        run_windowsize=true
    fi

    print_header

    # Check prerequisites
    if ! check_prerequisites; then
        echo ""
        echo -e "${RED}Prerequisites check failed. Please install missing tools.${NC}"
        exit 1
    fi

    # Build project
    if [ "$skip_build" = false ]; then
        if ! build_project; then
            echo ""
            echo -e "${RED}Build failed. Cannot continue with tests.${NC}"
            exit 1
        fi
    else
        echo -e "${YELLOW}Skipping build (--skip-build specified)${NC}"
    fi

    # Track overall success
    local all_passed=true

    # Run integration tests
    if [ "$run_integration" = true ]; then
        if [ -f "integration-test.sh" ]; then
            if ! run_test_suite "./integration-test.sh" \
                "Integration Tests" \
                "/tmp/tftp-test-*/test-results.txt"; then
                all_passed=false
            fi
        else
            echo -e "${YELLOW}⚠ Integration test script not found, skipping${NC}"
        fi
    fi

    # Run windowsize tests
    if [ "$run_windowsize" = true ]; then
        if command_exists atftp; then
            if [ -f "windowsize-test.sh" ]; then
                if ! run_test_suite "./windowsize-test.sh" \
                    "Windowsize Tests (RFC 7440)" \
                    "/tmp/tftp-windowsize-test-*/windowsize-results.txt"; then
                    all_passed=false
                fi
            else
                echo -e "${YELLOW}⚠ Windowsize test script not found, skipping${NC}"
            fi
        else
            echo -e "${YELLOW}⚠ atftp not found, skipping windowsize tests${NC}"
            echo -e "${YELLOW}  Install with: sudo apt-get install atftp${NC}"
        fi
    fi

    # Run performance analysis (Python)
    if [ "$run_performance" = true ]; then
        if command_exists python3 && [ -f "windowsize-analyzer.py" ]; then
            print_section "Performance Analysis"
            echo -e "${BLUE}Running performance comparison...${NC}"
            echo ""
            if python3 ./windowsize-analyzer.py performance; then
                echo ""
                echo -e "${GREEN}✓ Performance analysis completed${NC}"
            else
                echo ""
                echo -e "${RED}✗ Performance analysis failed${NC}"
                all_passed=false
            fi
        else
            echo -e "${YELLOW}⚠ python3 or windowsize-analyzer.py not found, skipping performance analysis${NC}"
        fi
    fi

    # Print final summary
    echo ""
    echo -e "${CYAN}╔════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║                                                ║${NC}"
    echo -e "${CYAN}║               FINAL TEST SUMMARY               ║${NC}"
    echo -e "${CYAN}║                                                ║${NC}"
    echo -e "${CYAN}╚════════════════════════════════════════════════╝${NC}"
    echo ""
    echo -e "  Total Tests:    $((TOTAL_PASSED + TOTAL_FAILED + TOTAL_SKIPPED))"
    echo -e "  ${GREEN}Passed:         $TOTAL_PASSED${NC}"
    echo -e "  ${RED}Failed:         $TOTAL_FAILED${NC}"
    echo -e "  ${YELLOW}Skipped:        $TOTAL_SKIPPED${NC}"
    echo ""

    if [ "$all_passed" = true ] && [ $TOTAL_FAILED -eq 0 ]; then
        echo -e "${GREEN}╔════════════════════════════════════════════════╗${NC}"
        echo -e "${GREEN}║                                                ║${NC}"
        echo -e "${GREEN}║          ✓ ALL TESTS PASSED! ✓                ║${NC}"
        echo -e "${GREEN}║                                                ║${NC}"
        echo -e "${GREEN}╚════════════════════════════════════════════════╝${NC}"
        exit 0
    else
        echo -e "${RED}╔════════════════════════════════════════════════╗${NC}"
        echo -e "${RED}║                                                ║${NC}"
        echo -e "${RED}║          ✗ SOME TESTS FAILED ✗                ║${NC}"
        echo -e "${RED}║                                                ║${NC}"
        echo -e "${RED}╚════════════════════════════════════════════════╝${NC}"
        exit 1
    fi
}

# Run main function
main "$@"
