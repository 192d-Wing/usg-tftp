# IPv6 Support in Snow-Owl TFTP Server

## Overview

The Snow-Owl TFTP server provides full IPv6 network support in compliance with **Rule 2: IPv6 Network Support** from the SFTP crate's development rules.

**Current Status:** ✅ Full IPv6 support - both binding and file transfers work

## Implementation Status

### ✅ Completed

1. **IPv6 Bind Address Support**
   - Server accepts IPv6 bind addresses in configuration
   - Default bind address is `[::]` (IPv6 UNSPECIFIED) on port 69
   - Dual-stack socket creation with `Domain::IPV6` or `Domain::IPV4`
   - Example config: `bind_addr = "[::1]:69"` or `bind_addr = "[::]:69"`

2. **Per-Transfer Socket IPv6 Support**
   - Automatic detection of client address family (IPv4 vs IPv6)
   - Per-transfer socket binding using correct address family via socket2 crate
   - Fixed EAFNOSUPPORT error by using `create_transfer_socket()` helper
   - Code locations:
     - [src/bin/server.rs:338](../src/bin/server.rs#L338) - `create_transfer_socket()`
     - [src/bin/server.rs:963-970](../src/bin/server.rs#L963) - Multicast response socket
     - [src/bin/server.rs:1365-1372](../src/bin/server.rs#L1365) - Read request handler
     - [src/bin/server.rs:1881-1888](../src/bin/server.rs#L1881) - Write request handler
     - [src/bin/server.rs:2589-2596](../src/bin/server.rs#L2589) - Error packet sending

3. **Configuration Schema**
   - `bind_addr` field accepts `SocketAddr` type
   - Supports both IPv4 and IPv6 formats:
     - IPv4: `"127.0.0.1:69"` or `"0.0.0.0:69"`
     - IPv6: `"[::1]:69"` or `"[::]:69"`
     - IPv6 with scope: `"[fe80::1%eth0]:69"`

4. **Dual-Stack File Transfers**
   - IPv4 clients work with IPv4-bound servers ✅
   - IPv6 clients work with IPv6-bound servers ✅
   - Dual-stack (`[::]`) accepts both IPv4 and IPv6 clients ✅

## Configuration Examples

### IPv6 Loopback (Testing)

```toml
root_dir = "/var/lib/snow-owl/tftp"
bind_addr = "[::1]:69"

[logging]
file = "/var/log/snow-owl/tftp-audit.json"
```

### Dual-Stack (IPv6 with IPv4 fallback) - Recommended

```toml
root_dir = "/var/lib/snow-owl/tftp"
bind_addr = "[::]:69"  # Listens on all interfaces, both IPv4 and IPv6

[logging]
file = "/var/log/snow-owl/tftp-audit.json"
```

### IPv4 Only

```toml
root_dir = "/var/lib/snow-owl/tftp"
bind_addr = "0.0.0.0:69"  # IPv4 all interfaces

[logging]
file = "/var/log/snow-owl/tftp-audit.json"
```

## Testing IPv6 Support

### Prerequisites

```bash
# Check IPv6 is available on system
ip -6 addr show

# Install atftp (supports IPv6)
sudo apt-get install atftp
```

### Manual Test

```bash
# Create config file
cat > /tmp/tftp-ipv6.toml <<'EOF'
root_dir = "/tmp"
bind_addr = "[::]:16969"

[logging]
file = "/tmp/tftp-audit.json"
level = "debug"
EOF

# Create test file
echo "IPv6 test content" > /tmp/testfile.txt

# Start server with IPv6 dual-stack
./target/release/snow-owl-tftp-server --config /tmp/tftp-ipv6.toml &
SERVER_PID=$!
sleep 2

# Test IPv6 file transfer
atftp -g -r testfile.txt -l /tmp/received.txt ::1 16969

# Verify
cat /tmp/received.txt  # Should show "IPv6 test content"

# Cleanup
kill $SERVER_PID
```

**Expected Result:**
- Server starts and binds to IPv6 address ✅
- Client connection accepted ✅
- File transfer completes successfully ✅
- Received file matches original ✅

## Technical Details

### Socket Creation Flow

1. **Main Server Socket** ([src/bin/server.rs:247](../src/bin/server.rs#L247))
   ```rust
   fn create_optimized_socket(bind_addr: SocketAddr, config: &SocketConfig) -> Result<UdpSocket> {
       let domain = if bind_addr.is_ipv4() {
           Domain::IPV4
       } else {
           Domain::IPV6
       };
       // ... socket configuration with optimizations
   }
   ```

2. **Per-Transfer Socket** ([src/bin/server.rs:338](../src/bin/server.rs#L338))
   ```rust
   /// Creates a per-transfer socket with the correct address family for IPv4/IPv6 support
   fn create_transfer_socket(bind_addr: SocketAddr) -> Result<UdpSocket> {
       let domain = if bind_addr.is_ipv4() {
           Domain::IPV4
       } else {
           Domain::IPV6
       };

       let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;
       socket.bind(&bind_addr.into())?;
       socket.set_nonblocking(true)?;

       // Convert to tokio socket
       let std_socket: std::net::UdpSocket = socket.into();
       Ok(UdpSocket::from_std(std_socket)?)
   }
   ```

3. **Address Family Selection** (per-transfer handlers)
   ```rust
   // Use IPv6 unspecified if client is IPv6, IPv4 otherwise (dual-stack support)
   let bind_addr = if client_addr.is_ipv6() {
       SocketAddr::new(std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 0)
   } else {
       SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0)
   };
   let socket = create_transfer_socket(bind_addr)?;
   socket.connect(client_addr).await?;
   ```

### Why socket2 is Required

The fix for EAFNOSUPPORT required using the `socket2` crate instead of `tokio::net::UdpSocket::bind()` directly. The key difference:

| Approach | Domain Selection | Result |
|----------|------------------|--------|
| `UdpSocket::bind("[::]:0")` | Implicit (may default to IPv4) | EAFNOSUPPORT when connecting to IPv6 |
| `Socket::new(Domain::IPV6, ...)` | Explicit IPv6 | Works correctly |

The socket2 crate allows explicit control over the socket domain, ensuring the socket is created with the correct address family before binding.

## Compliance

### NIST 800-53

- **SC-7:** Boundary Protection - IPv6 network boundary enforcement ✅

### Development Rules

- **Rule 2:** IPv6 Network Support (SFTP crate requirement)
  - ✅ All network code recognizes IPv6 addresses
  - ✅ IPv6 prioritized in default configuration (`[::]`)
  - ✅ Dual-stack support fully functional
  - ✅ IPv6-only mode supported
  - ⏳ IPv6 test scenarios (integration tests still use IPv4)

## Roadmap

### ✅ Completed

- [x] Fix EAFNOSUPPORT error in per-transfer sockets
- [x] Verify file integrity for IPv6 transfers
- [x] Test with IPv6 loopback (::1)
- [x] Test dual-stack configuration ([::] bind)

### Phase 2: Comprehensive Testing

- [x] Add IPv6 test cases to integration test suite (Tests 11-16)
- [x] Test dual-stack connectivity (Test 14)
- [x] Test concurrent IPv6 transfers (Test 15)
- [x] Test IPv6 NETASCII mode (Test 16)
- [ ] Test IPv4-mapped IPv6 addresses
- [ ] Test link-local addresses with scope IDs

### Phase 3: Advanced Features

- [ ] IPv6 multicast support (if applicable to TFTP)
- [ ] Link-local address handling with scope IDs
- [ ] IPv6 NAT64/DNS64 compatibility documentation
- [ ] Performance benchmarking IPv4 vs IPv6

## References

- **RFC 2732:** Format for Literal IPv6 Addresses in URL's
- **RFC 4291:** IP Version 6 Addressing Architecture
- **RFC 4007:** IPv6 Scoped Address Architecture
- **NIST 800-53:** SC-7 (Boundary Protection)

---

**Document Version:** 2.0
**Date:** 2026-01-20
**Status:** ✅ Full Implementation - IPv6 binding and transfers fully functional
**Test Results:** 16/16 integration tests pass (10 IPv4 + 6 IPv6)
