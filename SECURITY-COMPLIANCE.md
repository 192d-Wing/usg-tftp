# NIST 800-53 and STIG Compliance Mapping

## Snow-Owl TFTP Server Security Controls

This document maps the security controls implemented in the Snow-Owl TFTP server to NIST 800-53 Rev 5 controls and DoD Application Security and Development STIG requirements.

## Executive Summary

The Snow-Owl TFTP server implements comprehensive security controls addressing:

- **Access Control (AC)**: Path validation, directory restrictions, least privilege
- **Audit and Accountability (AU)**: Comprehensive logging of security events
- **Configuration Management (CM)**: Secure configuration validation
- **System and Communications Protection (SC)**: DoS protection, boundary enforcement
- **System and Information Integrity (SI)**: Input validation, buffer overflow protection

## NIST 800-53 Control Mappings

### AC-3: Access Enforcement

**Implementation**: File path validation and directory boundary enforcement

**Locations**:

- `main.rs:844-912` - `validate_and_resolve_path()` function
- `multicast.rs:771-835` - `validate_file_path()` function
- `config.rs:136-164` - Configuration validation

**Details**:

- All file access is restricted to configured root directory
- Path canonicalization prevents directory traversal
- Symbolic links are explicitly rejected
- Parent directory boundary checks for non-existent files

**Evidence**: Lines enforce `starts_with(&canonical_root)` check

---

### AC-6: Least Privilege

**Implementation**: Read-only file access, restricted directory access

**Locations**:

- `main.rs:458-466` - WRQ (write requests) explicitly rejected
- `main.rs:850` - CM-7 control comment: "read-only access, no writes"
- `main.rs:895` - Boundary check for least privilege

**Details**:

- Server operates in read-only mode
- No write operations permitted (TFTP WRQ rejected)
- File access limited to minimal required permissions

**Evidence**: `TftpErrorCode::AccessViolation` returned for all write attempts

---

### AU-2: Audit Events

**Implementation**: Comprehensive structured audit logging with SIEM integration

**Locations**:

- `audit.rs` - Complete audit event module with 20+ event types
- `main.rs` - Audit logging throughout all TFTP operations
- `multicast.rs` - Multicast session audit events
- `config.rs:40` - `audit_enabled` configuration flag

**Details**:

Comprehensive audit events for all security-relevant operations:
- **Server lifecycle**: Startup, shutdown, configuration changes
- **Client operations**: Connection initiation, read requests, denied access
- **File transfers**: Started, completed, failed (with metrics)
- **Security violations**: Path traversal attempts, symlink access, file size limits
- **Protocol violations**: Invalid opcodes, write requests, unsupported modes
- **Multicast sessions**: Session creation, client join/remove, completion
- **Access control**: Authentication attempts, authorization failures

All events include:
- ISO 8601 timestamp
- Hostname/system identifier
- Service name (snow-owl-tftp)
- Severity level (info, warn, error)
- Client address and port
- Detailed contextual information
- Optional correlation IDs

**Evidence**:
- `audit.rs:1-571` - Complete audit event catalog
- `main.rs:460-467` - Read request audit logging
- `main.rs:513-522` - Path violation audit logging
- `multicast.rs:417-425` - Multicast client join audit logging

---

### AU-3: Content of Audit Records

**Implementation**: Structured JSON audit records with comprehensive context

**Locations**:

- `audit.rs:232-248` - CommonFields structure with required AU-3 fields
- `audit.rs:13-230` - Detailed event structures with full context
- `main.rs:1018-1051` - JSON logging format support

**Details**:

Each audit record includes NIST AU-3 required fields:
- **Date and time**: ISO 8601 timestamp with timezone
- **Type of event**: Structured event_type field (JSON tag)
- **Subject identity**: Client address, username (when applicable)
- **Outcome**: Success/failure status, error messages
- **Additional information**: Event-specific context

Event-specific audit data:
- **File transfers**: Filename, size, mode, block size, duration, blocks sent
- **Security violations**: Requested path, violation type, reason
- **Multicast sessions**: Session ID, multicast group, port, client count
- **Resource limits**: File size, max allowed, resource type

**Format**: JSON-structured logs for SIEM parsing
```json
{
  "event_type": "read_request",
  "timestamp": "2026-01-18T10:30:45.123Z",
  "hostname": "tftp-server-01",
  "service": "snow-owl-tftp",
  "severity": "info",
  "client_addr": "192.168.1.100:54321",
  "filename": "firmware.bin",
  "mode": "octet",
  "options": {"blksize": "1024", "timeout": "5"}
}
```

**Evidence**:
- `audit.rs:232-273` - CommonFields implementation
- `audit.rs:276-337` - AuditEvent::log() structured logging
- Log output includes all required AU-3 fields

---

### AU-6: Audit Review, Analysis, and Reporting

**Implementation**: SIEM-compatible structured logging

**Locations**:

- `audit.rs` - JSON-serializable audit events
- `config.rs:50-62` - LogFormat enum with JSON support
- `main.rs:1018-1051` - JSON logging configuration

**Details**:

SIEM Integration Features:
- **Structured JSON format**: All audit events serialize to JSON
- **Consistent schema**: Tagged enum with common fields
- **Log file output**: Configurable file-based logging
- **Standard severity levels**: info, warn, error for filtering
- **Correlation IDs**: Optional field for tracking related events

Configuration example:
```toml
[logging]
format = "json"  # Enable JSON structured logging
file = "/var/log/snow-owl/tftp-audit.json"
audit_enabled = true
level = "info"
```

SIEM Integration:
- Forward logs to Splunk, ELK, Datadog, or other SIEM platforms
- Use log shippers (Filebeat, Fluentd, Vector) to collect JSON logs
- Query by event_type, severity, client_addr, or any event field
- Build dashboards for security monitoring and compliance reporting

**Evidence**:
- `audit.rs:4-5` - Serde JSON serialization support
- `config.rs:54-62` - JSON log format configuration
- `main.rs:1020-1024` - JSON logger initialization

---

### AU-9: Protection of Audit Information

**Implementation**: Secure audit log storage and integrity

**Locations**:

- `config.rs:186-205` - Log file path validation
- `main.rs:1001-1051` - Non-blocking log appender with guard
- `audit.rs:276-295` - Error handling for audit failures

**Details**:

Audit log protection mechanisms:
- **File permissions**: Logs protected by OS filesystem permissions
- **Path validation**: Log file parent directory must exist and be writable
- **Non-blocking writes**: Separate thread for log I/O prevents DoS
- **Write guard**: RAII guard ensures logs are flushed on shutdown
- **Error resilience**: Failed serialization doesn't crash server

**Evidence**:
- `config.rs:200-204` - Log file write validation
- `main.rs:1010-1016` - Non-blocking appender with guard
- `audit.rs:283-287` - Graceful error handling for serialization

---

### AU-12: Audit Generation

**Implementation**: Automatic audit event generation throughout lifecycle

**Locations**:

- `audit.rs:276-295` - Automated event logging via AuditEvent::log()
- `main.rs:1053-1060` - Server startup audit
- Throughout codebase - Automatic audit calls at security boundaries

**Details**:

Automated audit generation for:
- **All file access attempts**: Read requests, denials, completions
- **All security violations**: Path traversal, access violations, symlinks
- **All protocol errors**: Invalid opcodes, write requests, violations
- **All resource limits**: File size exceeded, timeout, retries
- **All session events**: Multicast sessions, client join/leave

No manual audit triggers required - events automatically generated
by instrumentation at security-relevant code paths.

**Evidence**:
- `audit.rs:281-294` - AuditEvent::log() auto-generates tracing events
- `main.rs:460-467` - Automatic read request audit
- `main.rs:513-522` - Automatic security violation audit

---

### CM-6: Configuration Settings

**Implementation**: Comprehensive configuration validation

**Locations**:

- `config.rs:125-211` - `validate_config()` function
- `config.rs:10-18` - Configuration structure with defaults
- `main.rs:948` - Config validation before startup

**Details**:

- All configuration parameters validated before use
- Secure defaults (100MB file size limit)
- Mandatory validation of root directory permissions
- Network binding validation

**Evidence**:

```rust
pub max_file_size_bytes: u64, // Default: 104_857_600 (100 MB)
```

---

### CM-7: Least Functionality

**Implementation**: Minimal feature set, read-only operation

**Locations**:

- `main.rs:850` - Explicit CM-7 annotation
- `main.rs:349-351` - MAIL mode rejection (obsolete feature)
- `main.rs:458-466` - Write operations rejected

**Details**:

- Only Read Request (RRQ) supported
- Write Request (WRQ) rejected
- Obsolete MAIL mode rejected
- No unnecessary features enabled

**Evidence**: Server implements RFC 1350 read operations only

---

### SC-5: Denial of Service Protection

**Implementation**: Multiple DoS protection mechanisms

**Locations**:

- `main.rs:23-35` - Constants with resource limits
- `main.rs:564-580` - File size limits
- `main.rs:807-842` - String length validation (255 bytes max)
- `main.rs:285` - Fixed-size buffer allocation
- `multicast.rs:727-743` - Integer overflow protection

**Details**:

- Maximum file size: 100MB (configurable)
- Maximum string length: 255 bytes
- Maximum packet size: 65,468 bytes
- Maximum retries: 5
- Session timeout: 5 seconds
- Integer overflow protection using `checked_mul()`

**Evidence**:

```rust
const MAX_STRING_LENGTH: usize = 255;
const MAX_PACKET_SIZE: usize = 65468;
const MAX_RETRIES: u32 = 5;
```

---

### SC-7: Boundary Protection

**Implementation**: Network and filesystem boundary enforcement

**Locations**:

- `main.rs:275` - SC-7 control annotation
- `main.rs:849` - SC-7(12) filesystem boundary
- `main.rs:887-909` - Canonical path boundary checks

**Details**:

- Network boundaries enforced via bind address
- Filesystem boundaries via path canonicalization
- No access outside configured root directory

**Evidence**: Path must satisfy `starts_with(&canonical_root)`

---

### SC-23: Session Authenticity

**Implementation**: Session timeout and connection management

**Locations**:

- `main.rs:27-28` - Timeout controls
- `main.rs:34` - Retry limits
- `main.rs:611-668` - ACK timeout handling

**Details**:

- 5-second default timeout per operation
- 5 retry limit before connection termination
- Unique Transfer ID (TID) per session

**Evidence**:

```rust
const DEFAULT_TIMEOUT_SECS: u64 = 5;
const MAX_RETRIES: u32 = 5;
```

---

### SI-10: Information Input Validation

**Implementation**: Comprehensive input validation

**Locations**:

- `main.rs:318-343` - Packet validation
- `main.rs:798-842` - String parsing validation
- `main.rs:844-912` - Path validation
- `main.rs:354-395` - TFTP option validation
- `multicast.rs:730-743` - Arithmetic validation

**Details**:
Input validation includes:

- Packet size validation
- String length limits (255 bytes)
- UTF-8 encoding validation
- Null terminator validation
- Path traversal prevention
- Integer overflow prevention
- Block size limits (8-65464 bytes)
- Timeout limits (1-255 seconds)

**Evidence**: Every input is validated before use

---

## STIG Compliance Mappings

### V-222563: Applications must produce audit records

**Status**: COMPLIANT

**Implementation**: Comprehensive tracing/logging throughout

**Evidence**:

- `main.rs:278` - STIG V-222563 annotation
- All security events logged via `tracing` crate
- Structured logging with log levels (info, warn, error)

---

### V-222564: Applications must protect audit information

**Status**: COMPLIANT

**Implementation**:

- Audit logs protected by filesystem permissions
- Optional log file with validated path
- No user-controlled data in security logs without sanitization

**Evidence**: `config.rs:133-135` - STIG V-222564 annotation

---

### V-222566: Applications must validate configuration parameters

**Status**: COMPLIANT

**Implementation**: `validate_config()` function validates all parameters

**Evidence**: `config.rs:125-211` validates:

- Root directory exists and is readable
- Bind address has non-zero port
- Multicast port in valid range (1024-65535)
- Log file parent directory exists
- Multicast IP version matches address type

---

### V-222577: Applications must validate all input

**Status**: COMPLIANT

**Implementation**: Multiple layers of input validation

**Evidence**:

- `main.rs:804-806` - STIG V-222577 annotation for string parsing
- `main.rs:324-326` - Input validation for packets
- `main.rs:838-839` - Character encoding validation
- All user inputs validated before processing

---

### V-222578: Applications must protect from code injection/buffer overflow

**Status**: COMPLIANT

**Implementation**:

- Rust memory safety prevents buffer overflows
- String length validation prevents overflow
- Integer overflow protection via `checked_mul()`

**Evidence**:

- `main.rs:805` - STIG V-222578 annotation
- `multicast.rs:734-735` - Integer overflow protection
- Rust's type system prevents memory corruption

**Note**: Rust is memory-safe by design; buffer overflows are impossible without `unsafe` blocks (none used)

---

### V-222596: Applications must set session timeout limits

**Status**: COMPLIANT

**Implementation**: 5-second session timeout enforced

**Evidence**: `main.rs:29-30` - STIG V-222596 annotation

```rust
const DEFAULT_TIMEOUT_SECS: u64 = 5;
```

---

### V-222597: Applications must limit retry attempts

**Status**: COMPLIANT

**Implementation**: Maximum 5 retry attempts

**Evidence**: `main.rs:30` - STIG V-222597 annotation

```rust
const MAX_RETRIES: u32 = 5;
```

---

### V-222602: Applications must enforce access restrictions

**Status**: COMPLIANT

**Implementation**: Path validation enforces directory boundaries

**Evidence**:

- `main.rs:853` - STIG V-222602 annotation
- `main.rs:887-909` - Access restriction enforcement
- `multicast.rs:779` - Multicast path access restrictions
- `config.rs:135` - Configuration access validation

---

### V-222603: Applications must protect against directory traversal

**Status**: COMPLIANT

**Implementation**: Multi-layered path traversal protection

**Evidence**:

- `main.rs:854` - STIG V-222603 annotation
- `main.rs:859-864` - ".." pattern rejection
- `main.rs:887-909` - Canonical path validation
- Symlink rejection prevents traversal via links

---

### V-222604: Applications must validate file paths

**Status**: COMPLIANT

**Implementation**: Comprehensive path validation

**Evidence**:

- `main.rs:855` - STIG V-222604 annotation
- `main.rs:872` - Symlink detection and rejection
- `main.rs:890-892` - Path canonicalization
- Parent directory validation for non-existent files

---

### V-222609: Applications must protect against resource exhaustion

**Status**: COMPLIANT

**Implementation**: Multiple resource limits

**Evidence**:

- `main.rs:569-570` - STIG V-222609 annotation
- `main.rs:811` - String length limit prevents resource exhaustion
- `main.rs:564-580` - File size limit (100MB default)
- `main.rs:285` - Fixed-size buffer allocation
- `multicast.rs:734` - Integer overflow protection

---

### V-222610: Applications must implement resource allocation restrictions

**Status**: COMPLIANT

**Implementation**: Configurable resource limits

**Evidence**:

- `main.rs:570` - STIG V-222610 annotation
- `config.rs:15-17` - `max_file_size_bytes` configuration
- Default 100MB file size limit
- Can be set to 0 for unlimited (documented as not recommended)

---

### V-222611: Applications must prevent unauthorized file access

**Status**: COMPLIANT

**Implementation**: Strict file access controls

**Evidence**:

- `main.rs:856` - STIG V-222611 annotation
- `main.rs:867` - Root directory restriction
- `main.rs:887-909` - Boundary enforcement
- Read-only access model

---

### V-222612: Applications must implement path canonicalization

**Status**: COMPLIANT

**Implementation**: Path canonicalization for all file operations

**Evidence**:

- `main.rs:857` - STIG V-222612 annotation
- `main.rs:881-883` - Root directory canonicalization
- `main.rs:887` - File path canonicalization
- `main.rs:894-898` - Parent directory canonicalization
- `multicast.rs:812-816` - Multicast path canonicalization

---

## Security Vulnerabilities Fixed

### 1. Path Traversal via TOCTOU (HIGH)

**NIST Controls**: AC-3, SI-10, SC-7(12)
**STIG**: V-222602, V-222603, V-222604, V-222612

**Fix**: Added symlink detection before file open operations

- `main.rs:864-876` - `symlink_metadata()` check
- Prevents race condition between validation and file open

---

### 2. Integer Overflow in Multicast (HIGH)

**NIST Controls**: SI-10, SC-5
**STIG**: V-222577, V-222578

**Fix**: Used `checked_mul()` for arithmetic operations

- `multicast.rs:730-743` - Overflow detection
- Prevents incorrect file offset calculations

---

### 3. Memory Exhaustion (HIGH)

**NIST Controls**: SC-5, SI-10
**STIG**: V-222609, V-222610

**Fix**: File size validation before reading into memory

- `main.rs:564-580` - File size check
- `config.rs:15-17` - Configurable maximum (100MB default)
- Prevents OOM attacks

---

### 4. String Length DoS (MEDIUM)

**NIST Controls**: SI-10, SC-5
**STIG**: V-222577, V-222609

**Fix**: 255-byte limit on TFTP strings

- `main.rs:812-833` - String length validation
- Prevents CPU/memory exhaustion from long strings

---

## Configuration Security

### Secure Defaults

```toml
max_file_size_bytes = 104857600  # 100 MB
bind_addr = "[::]:69"            # Standard TFTP port
root_dir = "/var/lib/snow-owl/tftp"  # Dedicated directory
```

### Required Security Configuration

1. **Root Directory**: Must be absolute path, must exist, must be readable
2. **File Size Limit**: Recommended to keep default 100MB or lower
3. **Network Binding**: Should bind to specific interface in production
4. **Logging**: Enable file logging for audit trails

### Validation on Startup

All configuration parameters are validated before server starts:

- `main.rs:948` - `validate_config(&config, true)?`
- Prevents operation with insecure configuration

---

## Security Testing Recommendations

### Path Traversal Testing

```bash
# Test directory traversal rejection
tftp localhost -c get "../../../etc/passwd"
tftp localhost -c get "..\\..\\..\\windows\\system32\\config\\sam"

# Test symlink rejection
ln -s /etc/passwd /var/lib/snow-owl/tftp/link
tftp localhost -c get "link"  # Should be rejected
```

### Resource Exhaustion Testing

```bash
# Test file size limit
dd if=/dev/zero of=/var/lib/snow-owl/tftp/large.bin bs=1M count=101
tftp localhost -c get "large.bin"  # Should reject >100MB

# Test concurrent connections
for i in {1..100}; do
  tftp localhost -c get "test.txt" &
done
```

### Integer Overflow Testing (Multicast)

- Test with block numbers near u16::MAX (65535)
- Test with maximum block size (65464 bytes)
- Verify no crashes or incorrect data

---

## Continuous Compliance

### Code Review Checklist

- [ ] All user input is validated (SI-10)
- [ ] File paths are canonicalized (V-222612)
- [ ] Access boundaries are enforced (AC-3)
- [ ] Resource limits are applied (SC-5)
- [ ] Security events are logged (AU-2, AU-3)
- [ ] No write operations permitted (CM-7)

### Automated Testing

- Run `cargo test` for unit tests including security validation
- Use `cargo clippy` for security lints
- Run `cargo audit` for dependency vulnerabilities

---

## References

- **NIST 800-53 Rev 5**: <https://csrc.nist.gov/publications/detail/sp/800-53/rev-5/final>
- **DoD Application Security and Development STIG**: <https://public.cyber.mil/stigs/>
- **RFC 1350**: TFTP Protocol Specification
- **RFC 2347**: TFTP Option Extension
- **RFC 2348**: TFTP Blocksize Option
- **RFC 2349**: TFTP Timeout Interval and Transfer Size Options
- **RFC 2090**: TFTP Multicast Option

---

## Compliance Summary

| Control Family | Controls Implemented | Compliance |
|---------------|---------------------|------------|
| Access Control (AC) | AC-3, AC-6 | ✅ COMPLIANT |
| Audit and Accountability (AU) | AU-2, AU-3 | ✅ COMPLIANT |
| Configuration Management (CM) | CM-6, CM-7 | ✅ COMPLIANT |
| System and Communications Protection (SC) | SC-5, SC-7, SC-23 | ✅ COMPLIANT |
| System and Information Integrity (SI) | SI-10 | ✅ COMPLIANT |

| STIG Requirement | Status |
|-----------------|--------|
| V-222563: Audit records | ✅ COMPLIANT |
| V-222564: Protect audit info | ✅ COMPLIANT |
| V-222566: Validate config | ✅ COMPLIANT |
| V-222577: Validate input | ✅ COMPLIANT |
| V-222578: Buffer overflow protection | ✅ COMPLIANT |
| V-222596: Session timeouts | ✅ COMPLIANT |
| V-222597: Retry limits | ✅ COMPLIANT |
| V-222602: Enforce access restrictions | ✅ COMPLIANT |
| V-222603: Directory traversal protection | ✅ COMPLIANT |
| V-222604: File path validation | ✅ COMPLIANT |
| V-222609: Resource exhaustion protection | ✅ COMPLIANT |
| V-222610: Resource allocation restrictions | ✅ COMPLIANT |
| V-222611: Prevent unauthorized access | ✅ COMPLIANT |
| V-222612: Path canonicalization | ✅ COMPLIANT |

**Overall Compliance: 100% of applicable controls implemented**

Last Updated: 2026-01-18
