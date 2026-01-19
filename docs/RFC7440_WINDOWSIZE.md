# RFC 7440 Windowsize Option Implementation

## Overview

This document describes the implementation of RFC 7440 "TFTP Windowsize Option" in Snow-Owl TFTP server. RFC 7440 extends the traditional TFTP stop-and-wait protocol with a sliding window mechanism, significantly improving throughput on high-latency networks.

## RFC 7440 Specification Summary

**RFC 7440** defines the `windowsize` option that allows the client and server to negotiate a window of consecutive blocks to send before waiting for an acknowledgment.

### Key Requirements

- **Option Name**: `windowsize` (case-insensitive)
- **Valid Range**: 1-65535 blocks (inclusive)
- **Default**: 1 (RFC 1350 compatible stop-and-wait behavior)
- **Negotiation**: Client proposes a value; server must accept it or negotiate down
- **ACK Behavior**: Receiver acknowledges only the **last block** in each window

### Protocol Behavior

#### Traditional TFTP (windowsize=1):
```
Server → DATA#1 [WAIT] ← ACK#1
Server → DATA#2 [WAIT] ← ACK#2
Server → DATA#3 [WAIT] ← ACK#3
```

#### RFC 7440 Windowed TFTP (windowsize=4):
```
Server → DATA#1
Server → DATA#2
Server → DATA#3
Server → DATA#4 [WAIT] ← ACK#4
Server → DATA#5
Server → DATA#6
Server → DATA#7
Server → DATA#8 [WAIT] ← ACK#8
```

## Implementation Details

### 1. TftpOptions Structure

Added `windowsize` field to the `TftpOptions` structure:

```rust
pub(crate) struct TftpOptions {
    pub block_size: usize,              // RFC 2348
    pub timeout: u64,                   // RFC 2349
    pub transfer_size: Option<u64>,     // RFC 2349
    pub windowsize: usize,              // RFC 7440 (NEW)
}
```

**Default Value**: 1 (RFC 1350 compatible)

### 2. Option Negotiation

#### Read Requests (RRQ)

The server parses the `windowsize` option from the client's RRQ packet:

```rust
"windowsize" => {
    match value.parse::<usize>() {
        Ok(size) if (1..=65535).contains(&size) => {
            options.windowsize = size;
            negotiated_options.insert("windowsize".to_string(), size.to_string());
        }
        Ok(size) => {
            warn!("Invalid windowsize={}, using default {}", size, options.windowsize);
        }
        Err(_) => {
            warn!("Non-numeric windowsize='{}', using default {}", value, options.windowsize);
        }
    }
}
```

#### Write Requests (WRQ)

Identical validation logic is applied for WRQ packets.

#### OACK Response

The `build_oack_packet()` function automatically includes `windowsize` in the Option Acknowledgment (OACK) when negotiated:

```
OACK Packet:
  +------+----------+-----+----------+-----+
  |  06  | blksize  |  0  |   8192   |  0  |
  +------+----------+-----+----------+-----+
  | windowsize |  0  |    16    |  0  |
  +------------+-----+----------+-----+
```

### 3. Sliding Window Data Transmission (Read Transfers)

#### Buffered Mode (Small Files)

For small NETASCII files (`< 1MB`), the implementation:

1. **Builds a window** of consecutive DATA packets
2. **Sends all packets** in the window without waiting
3. **Waits for ACK** of the last block in the window
4. **Retransmits entire window** on timeout or duplicate ACK

Key code structure:

```rust
while offset < file_data.len() {
    // Build window of packets
    let mut window_packets = Vec::with_capacity(windowsize);
    for _ in 0..windowsize {
        // Create DATA packet for block
        window_packets.push((block_num, packet, bytes_sent));
    }

    // Send all packets in window
    for (_, packet, _) in &window_packets {
        socket.send(packet).await?;
    }

    // Wait for ACK of last block only
    wait_for_ack(socket, last_block_in_window, timeout).await?;
}
```

#### Streaming Mode (Large Files)

For large files and OCTET mode:

1. **Reads blocks incrementally** from file
2. **Buffers a window** of blocks in memory
3. **Sends windowed packets** and waits for ACK
4. **Continues reading** and sending subsequent windows

This approach balances memory usage with performance.

### 4. Sliding Window ACK Handling (Write Transfers)

For write requests, the server (receiver):

1. **Receives multiple DATA packets** in sequence
2. **Only sends ACK** when:
   - The last block in a window is received, OR
   - The final block (< block_size) is received

```rust
// Calculate position in current window
let blocks_in_current_window = (block_num - 1) % windowsize as u16 + 1;
let should_ack = blocks_in_current_window == windowsize as u16 || is_final_block;

if should_ack {
    // Send ACK for the last block in window
    let mut ack_packet = BytesMut::with_capacity(4);
    ack_packet.put_u16(TftpOpcode::Ack as u16);
    ack_packet.put_u16(block_num);
    socket.send(&ack_packet).await?;
}
```

### 5. Configuration

Added `default_windowsize` to `PerformanceConfig`:

```rust
pub struct PerformanceConfig {
    pub default_block_size: usize,
    pub default_windowsize: usize,  // NEW: RFC 7440
    // ... other fields
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            default_block_size: 8192,
            default_windowsize: 1,  // RFC 1350 compatible by default
            // ...
        }
    }
}
```

**Configuration Example** (`tftp.toml`):

```toml
[performance]
default_block_size = 8192
default_windowsize = 16  # Recommended for typical networks
```

## Performance Impact

### Throughput Improvement

RFC 7440 significantly reduces the impact of round-trip time (RTT) on throughput:

**Traditional TFTP (windowsize=1)**:
```
Throughput ≈ BlockSize / RTT
```

**Windowed TFTP (windowsize=N)**:
```
Throughput ≈ (BlockSize × WindowSize) / RTT
```

### Example Calculation

Network: 100ms RTT, 8KB block size

| Windowsize | Throughput      | Speed      |
|------------|-----------------|------------|
| 1          | 8KB / 0.1s      | ~80 KB/s   |
| 4          | 32KB / 0.1s     | ~320 KB/s  |
| 16         | 128KB / 0.1s    | ~1.28 MB/s |
| 64         | 512KB / 0.1s    | ~5.12 MB/s |

### Recommended Settings

| Network Type                | Recommended Windowsize |
|-----------------------------|------------------------|
| Local LAN (< 5ms RTT)       | 1-4                    |
| Campus Network (5-20ms)     | 4-8                    |
| WAN / Internet (20-100ms)   | 16-32                  |
| High-latency satellite link | 64-128                 |

## Error Handling

### Out-of-Sequence Blocks

RFC 7440 specifies: *"the receiver SHOULD notify the sender by sending an ACK corresponding to the last data block correctly received."*

The implementation retransmits the entire window on:
- **Timeout** waiting for ACK
- **Duplicate ACK** (indicates packet loss)
- **Out-of-order ACK** (block number mismatch)

### Equivalence Guarantee

RFC 7440 requires: *"Traffic with windowsize = 1 MUST be equivalent to traffic specified by RFC 1350."*

The implementation ensures:
- `windowsize=1` behaves identically to traditional TFTP
- Backward compatibility with RFC 1350 clients
- Default configuration uses `windowsize=1`

## Security Considerations

### Denial of Service (DoS)

Large windowsize values can amplify bandwidth consumption. The server:
- **Validates windowsize** ≤ 65535 (RFC limit)
- **Rejects invalid values** and falls back to default
- **Can negotiate down** if client requests excessive windowsize

### Resource Exhaustion

Each window requires buffering multiple packets in memory. The server:
- **Limits window buffer size** to configured maximum
- **Validates file sizes** against `max_file_size_bytes` before windowing
- **Uses streaming mode** for large files to minimize memory usage

## Testing

### Manual Testing

Test windowsize negotiation with a TFTP client:

```bash
# Example using tftp-hpa client (may not support windowsize)
tftp -c get -l test.bin -r test.bin 192.168.1.10

# For full RFC 7440 testing, use a compatible client:
# - atftp with windowsize support
# - Custom test client
```

### Test Scenarios

1. ✅ **windowsize=1** (RFC 1350 compatibility)
2. ✅ **windowsize=4** (small window)
3. ✅ **windowsize=16** (typical window)
4. ✅ **windowsize=65535** (maximum valid value)
5. ✅ **Invalid windowsize** (0, 65536, negative, non-numeric)
6. ✅ **Packet loss simulation** (verify retransmission)
7. ✅ **Large file transfer** (streaming mode with windowing)

## References

- **RFC 7440**: TFTP Windowsize Option
  - https://datatracker.ietf.org/doc/html/rfc7440
- **RFC 1350**: The TFTP Protocol (Revision 2)
- **RFC 2347**: TFTP Option Extension
- **RFC 2348**: TFTP Blocksize Option
- **RFC 2349**: TFTP Timeout Interval and Transfer Size Options

## Implementation Status

| Feature | Status | Notes |
|---------|--------|-------|
| Option parsing (RRQ) | ✅ Complete | Validates 1-65535 range |
| Option parsing (WRQ) | ✅ Complete | Same validation as RRQ |
| OACK generation | ✅ Complete | Automatic inclusion |
| Windowed transmission (buffered) | ✅ Complete | Small files |
| Windowed transmission (streaming) | ✅ Complete | Large files |
| Windowed ACK (write) | ✅ Complete | ACK last block only |
| Configuration defaults | ✅ Complete | `default_windowsize` |
| Error handling | ✅ Complete | Timeout, duplicate ACK |
| Performance testing | ⏳ Pending | Benchmark needed |

## Future Enhancements

1. **Adaptive windowing**: Dynamically adjust windowsize based on packet loss
2. **Selective retransmission**: Retransmit only lost blocks (not entire window)
3. **Congestion control**: Implement RFC 5405 guidelines
4. **Performance metrics**: Log throughput improvements with windowing
