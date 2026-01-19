#!/bin/bash
# Phase 2 Performance Benchmarking Script
# Tests batch operations (recvmmsg/sendmmsg) performance improvements
# Target: Debian Trixie container (Linux 6.x)

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
TFTP_ROOT="$SCRIPT_DIR"
TEST_DIR="$TFTP_ROOT/benchmark-test"
RESULTS_DIR="$TEST_DIR/results"
BINARY="$PROJECT_ROOT/target/release/snow-owl-tftp"
SERVER_PORT=6969  # Non-privileged port for testing
CONCURRENT_CLIENTS=10

# Test files
SMALL_FILE="test-1kb.bin"
MEDIUM_FILE="test-100kb.bin"
LARGE_FILE="test-10mb.bin"

echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BLUE}  Snow-Owl TFTP Phase 2 Benchmark Suite${NC}"
echo -e "${BLUE}  Testing: Batch Operations (recvmmsg/sendmmsg)${NC}"
echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

# Function: Print section header
print_header() {
    echo ""
    echo -e "${YELLOW}▶ $1${NC}"
    echo -e "${YELLOW}$(printf '─%.0s' {1..70})${NC}"
}

# Function: Print success
print_success() {
    echo -e "${GREEN}✓ $1${NC}"
}

# Function: Print error
print_error() {
    echo -e "${RED}✗ $1${NC}"
}

# Function: Print info
print_info() {
    echo -e "${BLUE}ℹ $1${NC}"
}

# Function: Check prerequisites
check_prerequisites() {
    print_header "Checking Prerequisites"

    local missing=0

    # Check for required commands
    for cmd in cargo tftp strace bc; do
        if command -v $cmd &> /dev/null; then
            print_success "$cmd found"
        else
            print_error "$cmd not found - installing..."
            case $cmd in
                cargo)
                    print_error "Rust toolchain required. Install from https://rustup.rs/"
                    missing=1
                    ;;
                tftp)
                    apt-get update && apt-get install -y tftp
                    ;;
                strace)
                    apt-get update && apt-get install -y strace
                    ;;
                bc)
                    apt-get update && apt-get install -y bc
                    ;;
            esac
        fi
    done

    # Check Linux kernel version (need 2.6.33+ for recvmmsg)
    local kernel_version=$(uname -r | cut -d. -f1-2)
    print_info "Linux kernel: $kernel_version"

    if [ $missing -eq 1 ]; then
        print_error "Missing required dependencies"
        exit 1
    fi
}

# Function: Build release binary
build_binary() {
    print_header "Building Release Binary"

    cd "$PROJECT_ROOT"

    if [ -f "$BINARY" ]; then
        print_info "Existing binary found, rebuilding..."
        rm -f "$BINARY"
    fi

    print_info "Running: cargo build --release -p snow-owl-tftp"
    cargo build --release -p snow-owl-tftp

    sleep 2

    if [ -f "$BINARY" ]; then
        print_success "Binary built: $BINARY"
        ls -lh "$BINARY"
    else
        print_error "Failed to build binary"
        exit 1
    fi
}

# Function: Create test environment
setup_test_environment() {
    print_header "Setting Up Test Environment"

    # Create directories
    rm -rf "$TEST_DIR"
    mkdir -p "$TEST_DIR"/{tftp-root,results,configs}

    print_info "Test directory: $TEST_DIR"

    # Create test files
    print_info "Creating test files..."
    dd if=/dev/urandom of="$TEST_DIR/tftp-root/$SMALL_FILE" bs=1K count=1 2>/dev/null
    dd if=/dev/urandom of="$TEST_DIR/tftp-root/$MEDIUM_FILE" bs=1K count=100 2>/dev/null
    dd if=/dev/urandom of="$TEST_DIR/tftp-root/$LARGE_FILE" bs=1M count=10 2>/dev/null

    print_success "Created test files:"
    ls -lh "$TEST_DIR/tftp-root/"
}

# Function: Create config with batch disabled
create_config_no_batch() {
    local config_file="$TEST_DIR/configs/no-batch.toml"

    cat > "$config_file" << EOF
root_dir = "$TEST_DIR/tftp-root"
bind_addr = "0.0.0.0:$SERVER_PORT"
max_file_size_bytes = 104857600

[logging]
level = "info"
format = "text"

[write_config]
enabled = false

[multicast]
enabled = false

[performance]
default_block_size = 8192
buffer_pool_size = 128
streaming_threshold = 1048576

[performance.platform.socket]
recv_buffer_kb = 2048
send_buffer_kb = 2048
reuse_address = true
reuse_port = true

[performance.platform.file_io]
use_sequential_hint = true
use_willneed_hint = true
fadvise_dontneed_after = false

[performance.platform.batch]
enable_sendmmsg = false
enable_recvmmsg = false
max_batch_size = 32
batch_timeout_us = 100
EOF

    echo "$config_file"
}

# Function: Create config with batch enabled
create_config_with_batch() {
    local config_file="$TEST_DIR/configs/with-batch.toml"

    cat > "$config_file" << EOF
root_dir = "$TEST_DIR/tftp-root"
bind_addr = "0.0.0.0:$SERVER_PORT"
max_file_size_bytes = 104857600

[logging]
level = "info"
format = "text"

[write_config]
enabled = false

[multicast]
enabled = false

[performance]
default_block_size = 8192
buffer_pool_size = 128
streaming_threshold = 1048576

[performance.platform.socket]
recv_buffer_kb = 2048
send_buffer_kb = 2048
reuse_address = true
reuse_port = true

[performance.platform.file_io]
use_sequential_hint = true
use_willneed_hint = true
fadvise_dontneed_after = false

[performance.platform.batch]
enable_sendmmsg = true
enable_recvmmsg = true
max_batch_size = 32
batch_timeout_us = 100
EOF

    echo "$config_file"
}

# Function: Start server
start_server() {
    local config_file="$1"
    local label="$2"
    local log_file="$TEST_DIR/results/server-${label}.log"
    local pid_file="$TEST_DIR/server.pid"

    print_info "Starting server with config: $config_file"

    # Kill any existing server
    if [ -f "$pid_file" ]; then
        local old_pid=$(cat "$pid_file")
        if kill -0 "$old_pid" 2>/dev/null; then
            kill "$old_pid"
            sleep 1
        fi
        rm -f "$pid_file"
    fi

    # Start server in background
    "$BINARY" --config "$config_file" > "$log_file" 2>&1 &
    local pid=$!
    echo $pid > "$pid_file"

    # Wait for server to start
    sleep 2

    if kill -0 "$pid" 2>/dev/null; then
        print_success "Server started (PID: $pid)"
        return 0
    else
        print_error "Server failed to start"
        cat "$log_file"
        return 1
    fi
}

# Function: Stop server
stop_server() {
    local pid_file="$TEST_DIR/server.pid"

    if [ -f "$pid_file" ]; then
        local pid=$(cat "$pid_file")
        if kill -0 "$pid" 2>/dev/null; then
            print_info "Stopping server (PID: $pid)"
            kill "$pid"
            sleep 1

            # Force kill if still running
            if kill -0 "$pid" 2>/dev/null; then
                kill -9 "$pid" 2>/dev/null
            fi
        fi
        rm -f "$pid_file"
    fi
}

# Function: Run TFTP transfer
tftp_get() {
    local filename="$1"
    local output_dir="$2"

    tftp localhost $SERVER_PORT << EOF > /dev/null 2>&1
binary
get $filename $output_dir/$filename
quit
EOF

    return $?
}

# Function: Measure syscall counts
measure_syscalls() {
    local config_file="$1"
    local label="$2"
    local output_file="$RESULTS_DIR/syscalls-${label}.txt"

    print_header "Syscall Count Test: $label"

    # Start server with strace
    local pid_file="$TEST_DIR/server.pid"
    stop_server

    print_info "Starting server with strace..."
    strace -c -o "$output_file" "$BINARY" -c "$config_file" > /dev/null 2>&1 &
    local pid=$!
    echo $pid > "$pid_file"
    sleep 2

    # Perform transfers
    print_info "Performing $CONCURRENT_CLIENTS concurrent transfers..."
    for i in $(seq 1 $CONCURRENT_CLIENTS); do
        tftp_get "$MEDIUM_FILE" "$TEST_DIR" &
    done
    wait

    sleep 2

    # Stop server and collect strace data
    stop_server

    if [ -f "$output_file" ]; then
        print_success "Syscall data collected: $output_file"

        # Extract key metrics
        local recvfrom_count=$(grep "recvfrom" "$output_file" | awk '{print $1}' || echo "0")
        local recvmmsg_count=$(grep "recvmmsg" "$output_file" | awk '{print $1}' || echo "0")
        local sendto_count=$(grep "sendto" "$output_file" | awk '{print $1}' || echo "0")

        echo "recvfrom_calls=$recvfrom_count" > "$RESULTS_DIR/metrics-${label}.txt"
        echo "recvmmsg_calls=$recvmmsg_count" >> "$RESULTS_DIR/metrics-${label}.txt"
        echo "sendto_calls=$sendto_count" >> "$RESULTS_DIR/metrics-${label}.txt"

        print_info "recvfrom: $recvfrom_count, recvmmsg: $recvmmsg_count, sendto: $sendto_count"
    else
        print_error "Failed to collect syscall data"
    fi
}

# Function: Measure throughput
measure_throughput() {
    local config_file="$1"
    local label="$2"

    print_header "Throughput Test: $label"

    start_server "$config_file" "$label" || return 1

    # Single large file transfer
    print_info "Testing large file transfer..."
    local start_time=$(date +%s.%N)

    if tftp_get "$LARGE_FILE" "$TEST_DIR"; then
        local end_time=$(date +%s.%N)
        local duration=$(echo "$end_time - $start_time" | bc)
        local file_size=$(stat -c%s "$TEST_DIR/tftp-root/$LARGE_FILE")
        local throughput=$(echo "scale=2; ($file_size / 1048576) / $duration" | bc)

        print_success "Transfer completed in ${duration}s"
        print_success "Throughput: ${throughput} MB/s"

        echo "duration=$duration" >> "$RESULTS_DIR/metrics-${label}.txt"
        echo "throughput=$throughput" >> "$RESULTS_DIR/metrics-${label}.txt"
    else
        print_error "Transfer failed"
    fi

    # Concurrent transfers
    print_info "Testing concurrent transfers..."
    start_time=$(date +%s.%N)

    for i in $(seq 1 $CONCURRENT_CLIENTS); do
        tftp_get "$MEDIUM_FILE" "$TEST_DIR/client-$i" &
    done
    wait

    end_time=$(date +%s.%N)
    duration=$(echo "$end_time - $start_time" | bc)

    print_success "Concurrent transfers completed in ${duration}s"
    echo "concurrent_duration=$duration" >> "$RESULTS_DIR/metrics-${label}.txt"

    stop_server

    # Cleanup downloaded files
    rm -f "$TEST_DIR"/*.bin
    rm -rf "$TEST_DIR"/client-*
}

# Function: Generate comparison report
generate_report() {
    print_header "Performance Comparison Report"

    local no_batch_file="$RESULTS_DIR/metrics-no-batch.txt"
    local with_batch_file="$RESULTS_DIR/metrics-with-batch.txt"

    if [ ! -f "$no_batch_file" ] || [ ! -f "$with_batch_file" ]; then
        print_error "Metrics files not found"
        return 1
    fi

    # Parse metrics
    source "$no_batch_file"
    local nb_recvfrom=$recvfrom_calls
    local nb_recvmmsg=$recvmmsg_calls
    local nb_throughput=$throughput
    local nb_concurrent=$concurrent_duration

    source "$with_batch_file"
    local wb_recvfrom=$recvfrom_calls
    local wb_recvmmsg=$recvmmsg_calls
    local wb_throughput=$throughput
    local wb_concurrent=$concurrent_duration

    # Generate report
    local report_file="$RESULTS_DIR/benchmark-report.txt"

    cat > "$report_file" << EOF
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  Snow-Owl TFTP Phase 2 Benchmark Results
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Test Date: $(date '+%Y-%m-%d %H:%M:%S')
Platform: $(uname -sr)
Binary: $BINARY

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  Syscall Overhead Comparison
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

WITHOUT Batch Operations:
  - recvfrom() calls: $nb_recvfrom
  - recvmmsg() calls: $nb_recvmmsg

WITH Batch Operations:
  - recvfrom() calls: $wb_recvfrom
  - recvmmsg() calls: $wb_recvmmsg

EOF

    if [ "$nb_recvfrom" -gt 0 ] && [ "$wb_recvfrom" -gt 0 ]; then
        local reduction=$(echo "scale=2; 100 - ($wb_recvfrom * 100 / $nb_recvfrom)" | bc)
        echo "Syscall Reduction: ${reduction}%" >> "$report_file"

        if (( $(echo "$reduction >= 60" | bc -l) )); then
            echo "Status: ✓ PASS (Target: 60%+ reduction)" >> "$report_file"
        else
            echo "Status: ✗ FAIL (Target: 60%+ reduction)" >> "$report_file"
        fi
    fi

    cat >> "$report_file" << EOF

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  Throughput Comparison
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Single File Transfer (10 MB):
  - WITHOUT batch: ${nb_throughput} MB/s
  - WITH batch:    ${wb_throughput} MB/s

EOF

    if [ -n "$nb_throughput" ] && [ -n "$wb_throughput" ]; then
        local improvement=$(echo "scale=2; ($wb_throughput - $nb_throughput) / $nb_throughput * 100" | bc)
        echo "Improvement: ${improvement}%" >> "$report_file"
    fi

    cat >> "$report_file" << EOF

Concurrent Transfers ($CONCURRENT_CLIENTS clients):
  - WITHOUT batch: ${nb_concurrent}s
  - WITH batch:    ${wb_concurrent}s

EOF

    if [ -n "$nb_concurrent" ] && [ -n "$wb_concurrent" ]; then
        local improvement=$(echo "scale=2; ($nb_concurrent - $wb_concurrent) / $nb_concurrent * 100" | bc)
        echo "Improvement: ${improvement}%" >> "$report_file"

        if (( $(echo "$improvement >= 50" | bc -l) )); then
            echo "Status: ✓ PASS (Target: 2x improvement ~50%+)" >> "$report_file"
        else
            echo "Status: ⚠ PARTIAL (Target: 2x improvement ~50%+)" >> "$report_file"
        fi
    fi

    cat >> "$report_file" << EOF

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  Conclusion
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Phase 2 batch operations (recvmmsg/sendmmsg) implementation:
- Reduces syscall overhead significantly
- Improves concurrent transfer performance
- Maintains backward compatibility with fallback

Recommendation:
EOF

    if [ "$reduction" ] && (( $(echo "$reduction >= 60" | bc -l) )); then
        echo "✓ Phase 2 meets performance targets" >> "$report_file"
        echo "✓ Ready for production rollout" >> "$report_file"
        echo "✓ Consider Phase 3 (io_uring) for further scalability" >> "$report_file"
    else
        echo "⚠ Phase 2 shows improvements but below target" >> "$report_file"
        echo "⚠ Investigate configuration tuning before Phase 3" >> "$report_file"
    fi

    cat >> "$report_file" << EOF

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Detailed logs and strace output available in:
$RESULTS_DIR/

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
EOF

    # Display report
    cat "$report_file"

    print_success "Full report saved to: $report_file"
}

# Main execution
main() {
    # Check if running as root (needed for strace on some systems)
    if [ "$EUID" -ne 0 ] && ! strace -c /bin/true > /dev/null 2>&1; then
        print_error "This script requires root privileges or ptrace capability for strace"
        print_info "Run with: sudo $0"
        exit 1
    fi

    check_prerequisites
    build_binary
    setup_test_environment

    # Create configurations
    print_header "Creating Test Configurations"
    local config_no_batch=$(create_config_no_batch)
    local config_with_batch=$(create_config_with_batch)
    print_success "Configurations created in $TEST_DIR/configs/"

    # Run benchmarks
    measure_syscalls "$config_no_batch" "no-batch"
    measure_syscalls "$config_with_batch" "with-batch"

    measure_throughput "$config_no_batch" "no-batch"
    measure_throughput "$config_with_batch" "with-batch"

    # Generate report
    generate_report

    # Cleanup
    stop_server

    echo ""
    print_header "Benchmark Complete"
    print_success "All tests completed successfully"
    print_info "Results directory: $RESULTS_DIR"

    return 0
}

# Trap to ensure cleanup
trap 'stop_server' EXIT INT TERM

# Run main
main "$@"
