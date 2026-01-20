# IPv6 Support in Snow-Owl TFTP Server

## Overview

The Snow-Owl TFTP server includes infrastructure for IPv6 network support in compliance with **Rule 2: IPv6 Network Support** from the SFTP crate's development rules.

**Current Status:** Partial IPv6 support - binding works, transfers have compatibility issues

## Implementation Status

### ✅ Completed

1. **IPv6 Bind Address Support**
   - Server accepts IPv6 bind addresses in configuration
   - Default bind address is `[::]` (IPv6 UNSPECIFIED) on port 69
   - Dual-stack socket creation with `Domain::IPV6` or `Domain::IPV4`
   - Example config: `bind_addr = "[::1]:69"` or `bind_addr = "[::]:69"`

2. **Address Family Detection**
   - Automatic detection of client address family (IPv4 vs IPv6)
   - Per-transfer socket binding based on client address family
   - Code locations:
     - [src/main.rs:258](../src/main.rs#L258) - `create_optimized_socket()`
     - [src/main.rs:1084-1091](../src/main.rs#L1084) - Multicast response socket
     - [src/main.rs:1483-1490](../src/main.rs#L1483) - Read request handler
     - [src/main.rs:1997-2004](../src/main.rs#L1997) - Write request handler
     - [src/main.rs:2701-2708](../src/main.rs#L2701) - Error packet sending

3. **Configuration Schema**
   - `bind_addr` field accepts `SocketAddr` type
   - Supports both IPv4 and IPv6 formats:
     - IPv4: `"127.0.0.1:69"` or `"0.0.0.0:69"`
     - IPv6: `"[::1]:69"` or `"[::]:69"`
     - IPv6 with scope: `"[fe80::1%eth0]:69"`

### ⚠️ Known Issues

1. **IPv6 Transfer Failure (EAFNOSUPPORT)**
   - **Error:** "Address family not supported by protocol (os error 97)"
   - **Symptom:** Server binds to IPv6 successfully, accepts requests, but file transfers fail
   - **Root Cause:** Socket family mismatch when creating per-transfer sockets
   - **Workaround:** Use IPv4 addresses for now
   - **Status:** Under investigation

2. **Integration Tests IPv6 Coverage**
   - Current integration tests use IPv4 only (127.0.0.1)
   - No IPv6-specific test suite yet
   - Windowsize tests use IPv4

## Configuration Examples

### IPv6 Loopback (Testing)

```toml
root_dir = "/var/lib/snow-owl/tftp"
bind_addr = "[::1]:69"

[logging]
file = "/var/log/snow-owl/tftp-audit.json"
```

### Dual-Stack (IPv6 with IPv4 fallback)

```toml
root_dir = "/var/lib/snow-owl/tftp"
bind_addr = "[::]:69"  # Listens on all interfaces, both IPv4 and IPv6

[logging]
file = "/var/log/snow-owl/tftp-audit.json"
```

### IPv4 Only (Current Recommendation)

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
# Start server with IPv6 bind
./target/release/snow-owl-tftp-server --config /path/to/ipv6-config.toml

# Test file transfer (currently fails with EAFNOSUPPORT)
echo "test content" > /tmp/testfile.txt
atftp -g -r testfile.txt -l received.txt ::1 69
```

**Expected Result (Current):**
- Server starts and binds to IPv6 address ✅
- Client connection accepted ✅
- File transfer fails with "unknown error" ❌
- Audit log shows: "Address family not supported by protocol (os error 97)" ❌

## Technical Details

### Socket Creation Flow

1. **Main Server Socket** ([src/main.rs:257](../src/main.rs#L257))
   ```rust
   fn create_optimized_socket(bind_addr: SocketAddr, config: &SocketConfig) -> Result<UdpSocket> {
       let domain = if bind_addr.is_ipv4() {
           Domain::IPV4
       } else {
           Domain::IPV6
       };
       // ... socket configuration
   }
   ```

2. **Per-Transfer Socket** ([src/main.rs:1483-1490](../src/main.rs#L1483))
   ```rust
   // RFC 1350: Each transfer connection uses a new TID (Transfer ID)
   // Use IPv6 unspecified if client is IPv6, IPv4 otherwise (dual-stack support)
   let bind_addr = if client_addr.is_ipv6() {
       SocketAddr::new(std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 0)
   } else {
       SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0)
   };
   let socket = UdpSocket::bind(bind_addr).await?;
   socket.connect(client_addr).await?;
   ```

### Error Analysis

**EAFNOSUPPORT (Error 97)** occurs when:
- Attempting to bind an IPv4 socket and connect to IPv6 address
- Attempting to bind an IPv6 socket and connect to IPv4-mapped IPv6 address
- System doesn't support the requested address family

**Potential Fixes:**
1. Use `socket2` crate for better dual-stack control
2. Enable `IPV6_V6ONLY` socket option appropriately
3. Handle IPv4-mapped IPv6 addresses (`::ffff:192.0.2.1`)
4. Create socket with same family as server's bind address

## Compliance

### NIST 800-53

- **SC-7:** Boundary Protection - IPv6 network boundary enforcement

### Development Rules

- **Rule 2:** IPv6 Network Support (SFTP crate requirement)
  - ✅ All network code recognizes IPv6 addresses
  - ✅ IPv6 prioritized in default configuration (`[::]`)
  - ⚠️ Dual-stack support partial (binding works, transfers fail)
  - ❌ IPv6-only mode not yet functional
  - ❌ IPv6 test scenarios not yet implemented

## Roadmap

### Phase 1: Fix IPv6 Transfers (High Priority)

- [ ] Debug and fix EAFNOSUPPORT error in per-transfer sockets
- [ ] Verify file integrity for IPv6 transfers
- [ ] Test with various IPv6 address types (loopback, link-local, global)

### Phase 2: Dual-Stack Testing

- [ ] Add IPv6 test cases to integration test suite
- [ ] Test IPv4-mapped IPv6 addresses
- [ ] Test dual-stack configuration (`[::]` bind)
- [ ] Verify backward compatibility with IPv4-only clients

### Phase 3: IPv6-Only Mode

- [ ] Support IPv6-only deployments
- [ ] Document IPv6-only configuration
- [ ] Security considerations for IPv6-only networks

### Phase 4: Advanced Features

- [ ] IPv6 multicast support (if applicable to TFTP)
- [ ] Link-local address handling with scope IDs
- [ ] IPv6 NAT64/DNS64 compatibility
- [ ] Performance optimization for IPv6

## References

- **RFC 2732:** Format for Literal IPv6 Addresses in URL's
- **RFC 4291:** IP Version 6 Addressing Architecture
- **RFC 4007:** IPv6 Scoped Address Architecture
- **NIST 800-53:** SC-7 (Boundary Protection)

---

**Document Version:** 1.0
**Date:** 2026-01-20
**Status:** Partial Implementation - IPv6 binding works, transfers need fixing
**Priority:** Medium (IPv4 fully functional, IPv6 infrastructure in place)
