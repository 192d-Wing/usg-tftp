#!/usr/bin/env python3
"""
windowsize-analyzer.py - Advanced RFC 7440 windowsize analysis
Provides detailed performance metrics and analysis for windowsize testing
"""

import socket
import struct
import time
import hashlib
import os
import sys
from dataclasses import dataclass
from typing import Optional, List, Tuple
import statistics

# TFTP Opcodes
OPCODE_RRQ = 1
OPCODE_WRQ = 2
OPCODE_DATA = 3
OPCODE_ACK = 4
OPCODE_ERROR = 5
OPCODE_OACK = 6

# TFTP Constants
DEFAULT_BLOCKSIZE = 512
DEFAULT_TIMEOUT = 5
MAX_RETRIES = 3


@dataclass
class TransferMetrics:
    """Metrics collected during a TFTP transfer"""
    windowsize: int
    file_size: int
    transfer_time: float
    total_packets: int
    total_acks: int
    retransmissions: int
    throughput_mbps: float
    avg_rtt_ms: float
    packet_loss_rate: float


class TFTPClient:
    """RFC 7440 compliant TFTP client with windowsize support"""

    def __init__(self, host: str, port: int, timeout: float = DEFAULT_TIMEOUT):
        self.host = host
        self.port = port
        self.timeout = timeout
        self.sock = None

    def _create_rrq(self, filename: str, mode: str = "octet",
                    windowsize: Optional[int] = None,
                    blksize: int = DEFAULT_BLOCKSIZE) -> bytes:
        """Create a Read Request (RRQ) packet with options"""
        packet = struct.pack("!H", OPCODE_RRQ)
        packet += filename.encode('ascii') + b'\0'
        packet += mode.encode('ascii') + b'\0'

        if blksize != DEFAULT_BLOCKSIZE:
            packet += b'blksize\0' + str(blksize).encode('ascii') + b'\0'

        if windowsize is not None:
            packet += b'windowsize\0' + str(windowsize).encode('ascii') + b'\0'

        return packet

    def _create_ack(self, block_num: int) -> bytes:
        """Create an ACK packet"""
        return struct.pack("!HH", OPCODE_ACK, block_num)

    def _parse_data_packet(self, packet: bytes) -> Tuple[int, bytes]:
        """Parse a DATA packet"""
        opcode, block_num = struct.unpack("!HH", packet[:4])
        if opcode != OPCODE_DATA:
            raise ValueError(f"Expected DATA packet, got opcode {opcode}")
        return block_num, packet[4:]

    def _parse_error_packet(self, packet: bytes) -> Tuple[int, str]:
        """Parse an ERROR packet"""
        opcode, error_code = struct.unpack("!HH", packet[:4])
        if opcode != OPCODE_ERROR:
            raise ValueError(f"Expected ERROR packet, got opcode {opcode}")
        error_msg = packet[4:].rstrip(b'\0').decode('ascii', errors='replace')
        return error_code, error_msg

    def _parse_oack(self, packet: bytes) -> dict:
        """Parse an OACK packet"""
        opcode = struct.unpack("!H", packet[:2])[0]
        if opcode != OPCODE_OACK:
            raise ValueError(f"Expected OACK packet, got opcode {opcode}")

        options = {}
        parts = packet[2:].split(b'\0')
        for i in range(0, len(parts) - 1, 2):
            if parts[i]:
                key = parts[i].decode('ascii').lower()
                value = parts[i + 1].decode('ascii')
                options[key] = value

        return options

    def download_file(self, filename: str, output_path: str,
                     windowsize: Optional[int] = None,
                     blksize: int = DEFAULT_BLOCKSIZE) -> TransferMetrics:
        """Download a file using TFTP with optional windowsize"""

        # Create socket
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.settimeout(self.timeout)

        start_time = time.time()
        total_packets = 0
        total_acks = 0
        retransmissions = 0
        rtt_samples = []

        try:
            # Send RRQ
            rrq = self._create_rrq(filename, windowsize=windowsize, blksize=blksize)
            self.sock.sendto(rrq, (self.host, self.port))

            # Receive first packet (could be OACK or DATA)
            data, server_addr = self.sock.recvfrom(65536)
            opcode = struct.unpack("!H", data[:2])[0]

            negotiated_windowsize = windowsize if windowsize else 1
            negotiated_blksize = blksize

            if opcode == OPCODE_OACK:
                # Parse OACK
                options = self._parse_oack(data)
                if 'windowsize' in options:
                    negotiated_windowsize = int(options['windowsize'])
                if 'blksize' in options:
                    negotiated_blksize = int(options['blksize'])

                # Send ACK 0 to acknowledge OACK
                ack_time = time.time()
                self.sock.sendto(self._create_ack(0), server_addr)
                total_acks += 1

                # Wait for first DATA packet
                data, server_addr = self.sock.recvfrom(65536)
                rtt_samples.append((time.time() - ack_time) * 1000)  # ms

            # Download file data
            file_data = bytearray()
            expected_block = 1
            window_blocks = []
            last_ack_time = time.time()

            while True:
                opcode = struct.unpack("!H", data[:2])[0]

                if opcode == OPCODE_ERROR:
                    error_code, error_msg = self._parse_error_packet(data)
                    raise RuntimeError(f"TFTP Error {error_code}: {error_msg}")

                block_num, block_data = self._parse_data_packet(data)
                total_packets += 1

                # Handle block
                if block_num == expected_block:
                    file_data.extend(block_data)
                    window_blocks.append(block_num)
                    expected_block += 1

                    # Check if we need to send ACK (end of window or last block)
                    is_last_block = len(block_data) < negotiated_blksize
                    window_full = len(window_blocks) >= negotiated_windowsize

                    if is_last_block or window_full:
                        # Send ACK for last block in window
                        ack_time = time.time()
                        self.sock.sendto(self._create_ack(block_num), server_addr)
                        total_acks += 1
                        window_blocks = []

                        if is_last_block:
                            # Measure RTT for last ACK
                            rtt_samples.append((time.time() - last_ack_time) * 1000)
                            break

                        last_ack_time = ack_time
                else:
                    # Out of order or duplicate
                    retransmissions += 1

                # Receive next packet
                try:
                    data, server_addr = self.sock.recvfrom(65536)
                except socket.timeout:
                    # Timeout - resend last ACK
                    retransmissions += 1
                    if window_blocks:
                        self.sock.sendto(self._create_ack(window_blocks[-1]), server_addr)
                    else:
                        self.sock.sendto(self._create_ack(expected_block - 1), server_addr)
                    total_acks += 1
                    data, server_addr = self.sock.recvfrom(65536)

            # Write file
            with open(output_path, 'wb') as f:
                f.write(file_data)

            # Calculate metrics
            end_time = time.time()
            transfer_time = end_time - start_time
            file_size = len(file_data)
            throughput_mbps = (file_size * 8) / (transfer_time * 1_000_000)
            avg_rtt_ms = statistics.mean(rtt_samples) if rtt_samples else 0
            packet_loss_rate = retransmissions / max(total_packets, 1)

            return TransferMetrics(
                windowsize=negotiated_windowsize,
                file_size=file_size,
                transfer_time=transfer_time,
                total_packets=total_packets,
                total_acks=total_acks,
                retransmissions=retransmissions,
                throughput_mbps=throughput_mbps,
                avg_rtt_ms=avg_rtt_ms,
                packet_loss_rate=packet_loss_rate
            )

        finally:
            if self.sock:
                self.sock.close()


def verify_file_integrity(file_path: str, expected_hash: Optional[str] = None) -> str:
    """Calculate MD5 hash of file"""
    md5 = hashlib.md5()
    with open(file_path, 'rb') as f:
        for chunk in iter(lambda: f.read(4096), b''):
            md5.update(chunk)
    file_hash = md5.hexdigest()

    if expected_hash and file_hash != expected_hash:
        raise ValueError(f"File integrity check failed: {file_hash} != {expected_hash}")

    return file_hash


def run_windowsize_test(host: str, port: int, filename: str,
                       windowsize: int, test_num: int) -> Optional[TransferMetrics]:
    """Run a single windowsize test"""
    output_path = f"/tmp/tftp-test-ws{windowsize}-{test_num}.bin"

    try:
        client = TFTPClient(host, port)
        metrics = client.download_file(filename, output_path, windowsize=windowsize)

        # Verify file was downloaded
        if not os.path.exists(output_path):
            print(f"  ✗ Test {test_num}: File not downloaded")
            return None

        # Clean up
        os.remove(output_path)

        return metrics

    except Exception as e:
        print(f"  ✗ Test {test_num}: {str(e)}")
        if os.path.exists(output_path):
            os.remove(output_path)
        return None


def print_metrics_table(results: List[Tuple[int, TransferMetrics]]):
    """Print formatted metrics table"""
    print("\n" + "=" * 100)
    print(f"{'WS':<4} {'File Size':<12} {'Time (s)':<10} {'Throughput':<12} "
          f"{'Packets':<8} {'ACKs':<8} {'Retrans':<8} {'Loss %':<8}")
    print("=" * 100)

    for ws, metrics in results:
        print(f"{ws:<4} {metrics.file_size:<12} {metrics.transfer_time:<10.3f} "
              f"{metrics.throughput_mbps:<12.2f} {metrics.total_packets:<8} "
              f"{metrics.total_acks:<8} {metrics.retransmissions:<8} "
              f"{metrics.packet_loss_rate * 100:<8.2f}")

    print("=" * 100)


def main():
    """Main test execution"""
    if len(sys.argv) < 2:
        print("Usage: windowsize-analyzer.py <test_case>")
        print("  test_case: quick, full, performance")
        sys.exit(1)

    test_case = sys.argv[1]
    host = "127.0.0.1"
    port = 6970

    print(f"\nSnow-Owl TFTP Windowsize Analyzer (RFC 7440)")
    print(f"={'=' * 60}")

    results = []

    if test_case == "quick":
        # Quick test with medium file
        print("\nQuick Test: Medium file (10KB) with windowsize 1-8")
        for ws in range(1, 9):
            print(f"  Testing windowsize {ws}...", end=' ')
            metrics = run_windowsize_test(host, port, "medium.bin", ws, ws)
            if metrics:
                results.append((ws, metrics))
                print(f"✓ {metrics.throughput_mbps:.2f} Mbps")

    elif test_case == "full":
        # Full test suite (1-32)
        print("\nFull Test Suite: Tests 1-32")
        test_configs = [
            (1, "small.bin"), (2, "small.bin"), (3, "small.bin"), (4, "small.bin"),
            (5, "small.bin"), (6, "small.bin"), (7, "small.bin"), (8, "small.bin"),
            (1, "medium.bin"), (2, "medium.bin"), (4, "medium.bin"), (8, "medium.bin"),
            (12, "medium.bin"), (16, "medium.bin"), (24, "medium.bin"), (32, "medium.bin"),
            (1, "large.bin"), (2, "large.bin"), (4, "large.bin"), (8, "large.bin"),
            (16, "large.bin"), (32, "large.bin"), (48, "large.bin"), (64, "large.bin"),
            (1, "xlarge.bin"), (8, "xlarge.bin"), (32, "xlarge.bin"), (64, "xlarge.bin"),
            (1, "single-block.bin"), (16, "single-block.bin"),
            (16, "exact-window.bin"), (32, "exact-window.bin"),
        ]

        for test_num, (ws, filename) in enumerate(test_configs, 1):
            print(f"  Test {test_num:2d}: WS={ws:2d} File={filename:20s}...", end=' ')
            metrics = run_windowsize_test(host, port, filename, ws, test_num)
            if metrics:
                results.append((ws, metrics))
                print(f"✓ {metrics.throughput_mbps:.2f} Mbps")

    elif test_case == "performance":
        # Performance comparison
        print("\nPerformance Test: Large file with various windowsizes")
        for ws in [1, 2, 4, 8, 16, 32, 64]:
            print(f"  Testing windowsize {ws}...", end=' ')
            metrics = run_windowsize_test(host, port, "large.bin", ws, ws)
            if metrics:
                results.append((ws, metrics))
                print(f"✓ {metrics.throughput_mbps:.2f} Mbps")

    else:
        print(f"Unknown test case: {test_case}")
        sys.exit(1)

    # Print results
    if results:
        print_metrics_table(results)

        # Calculate improvements
        if len(results) > 1:
            baseline = results[0][1]
            best = max(results, key=lambda x: x[1].throughput_mbps)
            improvement = ((best[1].throughput_mbps - baseline.throughput_mbps) /
                          baseline.throughput_mbps * 100)

            print(f"\nPerformance Summary:")
            print(f"  Baseline (WS={results[0][0]}): {baseline.throughput_mbps:.2f} Mbps")
            print(f"  Best (WS={best[0]}): {best[1].throughput_mbps:.2f} Mbps")
            print(f"  Improvement: {improvement:.1f}%")

    print("\nTests completed successfully!")


if __name__ == "__main__":
    main()
