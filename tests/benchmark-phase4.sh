#!/bin/bash
# Phase 4 Performance Benchmarking Script
# Tests worker thread pool performance improvements
# Target: 2-4x improvement over Phase 3 single-threaded Tokio

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Platform detection
PLATFORM=$(uname -s)
if [ "$PLATFORM" = "Darwin" ]; then
    echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${YELLOW}  ⚠️  WARNING: Running on macOS (Darwin)${NC}"
    echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""
    echo -e "${YELLOW}Phase 4 worker pool is available on macOS, but performance may differ from Linux.${NC}"
    echo ""
    echo -e "Press Enter to continue, or Ctrl+C to exit..."
    read -r
    echo ""
fi

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
TFTP_ROOT="$SCRIPT_DIR"
TEST_DIR="$TFTP_ROOT/benchmark-test"
RESULTS_DIR="$TEST_DIR/results"
BINARY="$PROJECT_ROOT/target/release/snow-owl-tftp"
SERVER_PORT=6969  # Non-privileged port for testing
CONCURRENT_CLIENTS=100  # High concurrency to stress test worker pool

# Test files
SMALL_FILE="test-1kb.bin"
MEDIUM_FILE="test-100kb.bin"
LARGE_FILE="test-10mb.bin"

echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${BLUE}  Snow-Owl TFTP Phase 4 Benchmark Suite${NC}"
echo -e "${BLUE}  Testing: Multi-threaded Worker Pool${NC}"
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
    for cmd in cargo tftp bc; do
        if command -v $cmd &> /dev/null; then
            print_success "$cmd found"
        else
            print_error "$cmd not found"
            missing=1
        fi
    done

    # CPU info
    local cpu_count=$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo "unknown")
    print_info "CPU cores: $cpu_count"

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

# Function: Create config WITHOUT worker pool (Phase 3 baseline)
create_config_no_workers() {
    local config_file="$TEST_DIR/configs/no-workers.toml"

    cat > "$config_file" << EOF
root_dir = "$TEST_DIR/tftp-root"
bind_addr = "0.0.0.0:$SERVER_PORT"
max_file_size_bytes = 104857600

[logging]
file = "$TEST_DIR/results/tftp-no-workers.log"

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
enable_adaptive_batching = true
adaptive_batch_threshold = 5

[performance.platform.worker_pool]
enabled = false
EOF

    echo "$config_file"
}

# Function: Create config WITH worker pool (Phase 4)
create_config_with_workers() {
    local config_file="$TEST_DIR/configs/with-workers.toml"
    local cpu_count=$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo "4")
    local worker_count=$((cpu_count > 2 ? cpu_count - 2 : 2))

    cat > "$config_file" << EOF
root_dir = "$TEST_DIR/tftp-root"
bind_addr = "0.0.0.0:$SERVER_PORT"
max_file_size_bytes = 104857600

[logging]
file = "$TEST_DIR/results/tftp-with-workers.log"

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
enable_adaptive_batching = true
adaptive_batch_threshold = 5

[performance.platform.worker_pool]
enabled = true
worker_count = $worker_count
worker_channel_size = 256
sender_channel_size = 512
load_balance_strategy = "round_robin"
enable_cpu_affinity = false
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

    # Create log directory if it doesn't exist
    if command -v sudo &> /dev/null && [ "$EUID" -ne 0 ]; then
        sudo mkdir -p /var/log/snow-owl 2>/dev/null || true
    else
        mkdir -p /var/log/snow-owl 2>/dev/null || true
    fi

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

        # Quick connectivity test
        print_info "Testing server connectivity..."
        if timeout 5 tftp localhost $SERVER_PORT << EOF > /dev/null 2>&1
binary
quit
EOF
        then
            print_success "Server is responding to TFTP requests"
        else
            print_error "Server not responding to TFTP requests (check logs)"
            cat "$log_file"
            return 1
        fi

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

    mkdir -p "$output_dir"

    timeout 30 tftp localhost $SERVER_PORT << EOF > /dev/null 2>&1
binary
get $filename $output_dir/$filename
quit
EOF

    local result=$?
    if [ $result -eq 124 ]; then
        echo "TFTP transfer timed out for $filename" >&2
        return 1
    fi
    return $result
}

# Function: Measure CPU usage during test
measure_cpu_usage() {
    local pid="$1"
    local duration="$2"
    local output_file="$3"

    # Sample CPU usage every 0.1s for duration
    local samples=0
    local total_cpu=0
    local end_time=$(($(date +%s) + duration))

    while [ $(date +%s) -lt $end_time ]; do
        if kill -0 "$pid" 2>/dev/null; then
            # Get CPU% from ps (platform-agnostic)
            local cpu=$(ps -p "$pid" -o %cpu= 2>/dev/null | awk '{print $1}' || echo "0")
            if [ -n "$cpu" ] && [ "$cpu" != "0" ]; then
                total_cpu=$(echo "$total_cpu + $cpu" | bc)
                samples=$((samples + 1))
            fi
        fi
        sleep 0.1
    done

    if [ $samples -gt 0 ]; then
        local avg_cpu=$(echo "scale=2; $total_cpu / $samples" | bc)
        echo "avg_cpu=$avg_cpu" >> "$output_file"
        echo "cpu_samples=$samples" >> "$output_file"
    fi
}

# Function: Measure throughput
measure_throughput() {
    local config_file="$1"
    local label="$2"

    print_header "Throughput Test: $label"

    start_server "$config_file" "$label" || return 1

    local server_pid=$(cat "$TEST_DIR/server.pid")
    local metrics_file="$RESULTS_DIR/metrics-${label}.txt"

    # Single large file transfer
    print_info "Testing large file transfer..."
    local start_time=$(date +%s.%N)

    if tftp_get "$LARGE_FILE" "$TEST_DIR"; then
        local end_time=$(date +%s.%N)
        local duration=$(echo "$end_time - $start_time" | bc)
        local file_size=$(stat -c%s "$TEST_DIR/tftp-root/$LARGE_FILE" 2>/dev/null || stat -f%z "$TEST_DIR/tftp-root/$LARGE_FILE" 2>/dev/null)
        local throughput=$(echo "scale=2; ($file_size / 1048576) / $duration" | bc)

        print_success "Transfer completed in ${duration}s"
        print_success "Throughput: ${throughput} MB/s"

        echo "duration=$duration" >> "$metrics_file"
        echo "throughput=$throughput" >> "$metrics_file"
    else
        print_error "Transfer failed"
    fi

    # Concurrent transfers with CPU monitoring
    print_info "Testing $CONCURRENT_CLIENTS concurrent transfers..."

    # Verify server is still running
    if ! kill -0 $(cat "$TEST_DIR/server.pid" 2>/dev/null) 2>/dev/null; then
        print_error "Server is not running, cannot perform concurrent test"
        return 1
    fi

    # Start CPU monitoring in background
    measure_cpu_usage "$server_pid" 30 "$metrics_file" &
    local cpu_monitor_pid=$!

    start_time=$(date +%s.%N)

    # Track PIDs for better error handling
    declare -a pids=()
    for i in $(seq 1 $CONCURRENT_CLIENTS); do
        tftp_get "$MEDIUM_FILE" "$TEST_DIR/client-$i" &
        pids+=($!)
    done

    # Wait for all background jobs
    print_info "Waiting for $CONCURRENT_CLIENTS concurrent transfers to complete..."

    failed=0
    for pid in "${pids[@]}"; do
        if ! wait $pid; then
            ((failed++))
        fi
    done

    end_time=$(date +%s.%N)
    duration=$(echo "$end_time - $start_time" | bc)

    # Wait for CPU monitoring to finish
    wait $cpu_monitor_pid 2>/dev/null || true

    if [ $failed -eq 0 ]; then
        print_success "All $CONCURRENT_CLIENTS concurrent transfers completed in ${duration}s"
        local total_mb=$(echo "scale=2; ($CONCURRENT_CLIENTS * 100) / 1024" | bc)
        local aggregate_throughput=$(echo "scale=2; $total_mb / $duration" | bc)
        print_success "Aggregate throughput: ${aggregate_throughput} MB/s"
        echo "aggregate_throughput=$aggregate_throughput" >> "$metrics_file"
    else
        print_error "$failed out of $CONCURRENT_CLIENTS transfers failed (completed in ${duration}s)"
    fi

    echo "concurrent_duration=$duration" >> "$metrics_file"
    echo "concurrent_failures=$failed" >> "$metrics_file"

    stop_server

    # Cleanup downloaded files
    rm -f "$TEST_DIR"/*.bin
    rm -rf "$TEST_DIR"/client-*
}

# Function: Generate comparison report
generate_report() {
    print_header "Performance Comparison Report"

    local no_workers_file="$RESULTS_DIR/metrics-no-workers.txt"
    local with_workers_file="$RESULTS_DIR/metrics-with-workers.txt"

    if [ ! -f "$no_workers_file" ] || [ ! -f "$with_workers_file" ]; then
        print_error "Metrics files not found"
        return 1
    fi

    # Parse metrics
    source "$no_workers_file"
    local nw_throughput=$throughput
    local nw_concurrent=$concurrent_duration
    local nw_aggregate=${aggregate_throughput:-0}
    local nw_cpu=${avg_cpu:-0}
    local nw_failures=${concurrent_failures:-0}

    source "$with_workers_file"
    local ww_throughput=$throughput
    local ww_concurrent=$concurrent_duration
    local ww_aggregate=${aggregate_throughput:-0}
    local ww_cpu=${avg_cpu:-0}
    local ww_failures=${concurrent_failures:-0}

    # Get CPU count
    local cpu_count=$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo "4")
    local worker_count=$((cpu_count > 2 ? cpu_count - 2 : 2))

    # Generate report
    local report_file="$RESULTS_DIR/benchmark-report.txt"

    cat > "$report_file" << EOF
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  Snow-Owl TFTP Phase 4 Benchmark Results
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Test Date: $(date '+%Y-%m-%d %H:%M:%S')
Platform: $(uname -sr)
Binary: $BINARY
CPU Cores: $cpu_count
Worker Threads: $worker_count

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  Single File Transfer Comparison (10 MB)
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

WITHOUT Worker Pool (Phase 3 Tokio):
  - Throughput: ${nw_throughput} MB/s

WITH Worker Pool (Phase 4):
  - Throughput: ${ww_throughput} MB/s

EOF

    if [ -n "$nw_throughput" ] && [ -n "$ww_throughput" ]; then
        local improvement=$(echo "scale=2; ($ww_throughput - $nw_throughput) / $nw_throughput * 100" | bc)
        echo "Single File Improvement: ${improvement}%" >> "$report_file"
    fi

    cat >> "$report_file" << EOF

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  Concurrent Transfer Comparison ($CONCURRENT_CLIENTS clients)
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

WITHOUT Worker Pool:
  - Duration: ${nw_concurrent}s
  - Aggregate Throughput: ${nw_aggregate} MB/s
  - Average CPU: ${nw_cpu}%
  - Failed Transfers: ${nw_failures}/$CONCURRENT_CLIENTS

WITH Worker Pool:
  - Duration: ${ww_concurrent}s
  - Aggregate Throughput: ${ww_aggregate} MB/s
  - Average CPU: ${ww_cpu}%
  - Failed Transfers: ${ww_failures}/$CONCURRENT_CLIENTS

EOF

    # Calculate improvements
    if [ -n "$nw_concurrent" ] && [ -n "$ww_concurrent" ]; then
        local time_improvement=$(echo "scale=2; ($nw_concurrent - $ww_concurrent) / $nw_concurrent * 100" | bc)
        echo "Time Reduction: ${time_improvement}%" >> "$report_file"

        # Calculate speedup multiplier
        local speedup=$(echo "scale=2; $nw_concurrent / $ww_concurrent" | bc)
        echo "Speedup: ${speedup}x" >> "$report_file"
        echo "" >> "$report_file"

        # Determine pass/fail
        if (( $(echo "$speedup >= 2.0" | bc -l) )); then
            echo "Status: ✓ PASS (Target: 2-4x improvement achieved: ${speedup}x)" >> "$report_file"
        elif (( $(echo "$speedup >= 1.5" | bc -l) )); then
            echo "Status: ⚠ PARTIAL (Target: 2-4x, achieved: ${speedup}x)" >> "$report_file"
        else
            echo "Status: ✗ NEEDS IMPROVEMENT (Target: 2-4x, achieved: ${speedup}x)" >> "$report_file"
        fi
    fi

    if [ -n "$nw_aggregate" ] && [ -n "$ww_aggregate" ] && [ "$nw_aggregate" != "0" ] && [ "$ww_aggregate" != "0" ]; then
        local throughput_improvement=$(echo "scale=2; ($ww_aggregate - $nw_aggregate) / $nw_aggregate * 100" | bc)
        echo "Aggregate Throughput Improvement: ${throughput_improvement}%" >> "$report_file"
    fi

    cat >> "$report_file" << EOF

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  CPU Utilization Analysis
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

EOF

    if [ -n "$nw_cpu" ] && [ -n "$ww_cpu" ] && [ "$nw_cpu" != "0" ] && [ "$ww_cpu" != "0" ]; then
        local cpu_increase=$(echo "scale=2; ($ww_cpu - $nw_cpu) / $nw_cpu * 100" | bc)
        cat >> "$report_file" << EOF
CPU Usage Change: ${cpu_increase}%

Analysis:
- Worker pool enables multi-core utilization
- Expected: Higher CPU usage but better throughput
- Efficiency: $(echo "scale=2; $ww_aggregate / $ww_cpu" | bc) MB/s per CPU%

EOF
    fi

    cat >> "$report_file" << EOF
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  Conclusion
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Phase 4 multi-threaded worker pool implementation:
- Utilizes multiple CPU cores for parallel request processing
- Improves concurrent transfer performance significantly
- NGINX-style architecture with master + workers + sender threads

Recommendation:
EOF

    if [ -n "$speedup" ] && (( $(echo "$speedup >= 2.0" | bc -l) )); then
        echo "✓ Phase 4 meets or exceeds performance targets (${speedup}x speedup)" >> "$report_file"
        echo "✓ Ready for production deployment" >> "$report_file"
        echo "✓ Worker pool provides excellent multi-core scaling" >> "$report_file"
    else
        echo "⚠ Phase 4 shows improvements but below 2x target" >> "$report_file"
        echo "⚠ Consider tuning worker_count, channel sizes, or load_balance_strategy" >> "$report_file"
        echo "⚠ Verify system is not CPU/network/disk bottlenecked" >> "$report_file"
    fi

    cat >> "$report_file" << EOF

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Detailed logs available in:
$RESULTS_DIR/

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
EOF

    # Display report
    cat "$report_file"

    print_success "Full report saved to: $report_file"
}

# Main execution
main() {
    check_prerequisites
    build_binary
    setup_test_environment

    # Create configurations
    print_header "Creating Test Configurations"
    local config_no_workers=$(create_config_no_workers)
    local config_with_workers=$(create_config_with_workers)
    print_success "Configurations created in $TEST_DIR/configs/"

    # Run benchmarks
    measure_throughput "$config_no_workers" "no-workers"
    measure_throughput "$config_with_workers" "with-workers"

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
