# TFTP Write Operations

This document describes how to configure and use write operations in the Snow-Owl TFTP server.

## Overview

The Snow-Owl TFTP server supports RFC 1350 Write Request (WRQ) operations with additional security controls:

- **Disabled by default** - Write operations must be explicitly enabled
- **Pattern-based access control** - Only files matching allowed patterns can be written
- **Configurable overwrite protection** - Control whether existing files can be overwritten
- **Comprehensive audit logging** - All write attempts are logged for SIEM integration
- **NETASCII and OCTET modes** - Full support for both transfer modes
- **File size limits** - Protection against resource exhaustion attacks

## Security Model

Write operations follow defense-in-depth principles aligned with NIST 800-53 controls:

- **AC-3: Access Enforcement** - Pattern-based allow-lists restrict write access
- **AC-6: Least Privilege** - Minimal write permissions by default
- **CM-5: Access Restrictions for Change** - Controlled file modifications
- **AU-2: Audit Events** - Comprehensive logging of all write operations
- **SI-7: Software Integrity** - Atomic writes prevent partial file corruption

## Configuration

### Basic Configuration

To enable write operations, add the following to your `tftp.toml`:

```toml
[write_config]
enabled = true
allow_overwrite = false
allowed_patterns = [
    "*.txt",
    "configs/*.cfg",
    "firmware/device-*.bin"
]
```

### Configuration Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `false` | Enable or disable write operations globally |
| `allow_overwrite` | boolean | `false` | Allow overwriting existing files |
| `allowed_patterns` | array of strings | `[]` | Glob patterns for files that can be written |

### Pattern Syntax

Patterns use glob syntax for flexible matching:

- `*.txt` - Any `.txt` file in the root directory
- `**/*.bin` - Any `.bin` file in any subdirectory
- `configs/*.cfg` - Any `.cfg` file in the `configs` directory
- `firmware/device-*.bin` - Device firmware files matching pattern

**Security Note:** Overly permissive patterns (`*`, `**`, `**/*`) are rejected to prevent accidental exposure.

## Example Configurations

### Configuration Firmware Updates

```toml
root_dir = "/var/lib/tftp"
bind_addr = "[::]:69"

[write_config]
enabled = true
allow_overwrite = true  # Allow firmware updates
allowed_patterns = [
    "firmware/*.bin",
    "firmware/*.img"
]
```

### Network Device Configuration Backups

```toml
root_dir = "/var/lib/tftp/backups"
bind_addr = "[::]:69"

[write_config]
enabled = true
allow_overwrite = false  # Prevent accidental overwrites
allowed_patterns = [
    "configs/router-*.cfg",
    "configs/switch-*.cfg"
]
```

### Log Collection

```toml
root_dir = "/var/lib/tftp/logs"
bind_addr = "[::]:69"

[write_config]
enabled = true
allow_overwrite = true  # Allow log rotation
allowed_patterns = [
    "*.log",
    "devices/*/*.log"
]
```

## Audit Logging

All write operations generate structured audit logs suitable for SIEM integration:

### Successful Write

```json
{
  "event_type": "write_completed",
  "timestamp": "2026-01-19T12:34:56.789Z",
  "hostname": "tftp-server",
  "service": "snow-owl-tftp",
  "severity": "info",
  "client_addr": "192.168.1.100:49152",
  "filename": "firmware/router.bin",
  "bytes_received": 1048576,
  "blocks_received": 2048,
  "duration_ms": 5432,
  "throughput_bps": 193157,
  "avg_block_time_ms": 2.65,
  "file_created": true
}
```

### Denied Write Attempt

```json
{
  "event_type": "write_request_denied",
  "timestamp": "2026-01-19T12:34:56.789Z",
  "hostname": "tftp-server",
  "service": "snow-owl-tftp",
  "severity": "warn",
  "client_addr": "192.168.1.100:49152",
  "filename": "../../etc/passwd",
  "reason": "file not in allowed_patterns"
}
```

## Security Considerations

### Path Traversal Protection

The server validates all file paths to prevent directory traversal attacks:

- `..` sequences are rejected
- Symlinks are not followed
- Paths are canonicalized and checked against `root_dir`

### File Size Limits

Configure `max_file_size_bytes` to prevent resource exhaustion:

```toml
max_file_size_bytes = 104857600  # 100 MB limit
```

### Network Security

- Bind to specific interfaces to limit exposure
- Use firewall rules to restrict client access
- Monitor audit logs for suspicious activity

### File System Permissions

Ensure the TFTP server runs with minimal privileges:

```bash
# Create dedicated user
sudo useradd -r -s /bin/false tftp

# Set directory ownership
sudo chown -R tftp:tftp /var/lib/tftp

# Restrict permissions
sudo chmod 755 /var/lib/tftp
```

## Testing Write Operations

### Using tftp Client

```bash
# Upload a file
echo "test content" > test.txt
tftp 192.168.1.1 << EOF
mode octet
put test.txt
quit
EOF
```

### Using curl

```bash
# Upload via TFTP (requires tftp:// support)
curl -T firmware.bin tftp://192.168.1.1/firmware/device-01.bin
```

## Troubleshooting

### Write Request Denied

**Error:** "Write not supported"

**Cause:** Write operations are disabled in configuration

**Solution:** Set `write_config.enabled = true` in `tftp.toml`

---

**Error:** "File not allowed for writing"

**Cause:** Filename doesn't match any pattern in `allowed_patterns`

**Solution:** Add appropriate pattern to `allowed_patterns`

---

**Error:** "File already exists"

**Cause:** `allow_overwrite = false` and file exists

**Solution:** Set `allow_overwrite = true` or delete existing file

### File Size Issues

**Error:** "File too large"

**Cause:** File exceeds `max_file_size_bytes`

**Solution:** Increase `max_file_size_bytes` or upload smaller files

### Permission Issues

**Error:** "Write failed" (in server logs)

**Cause:** TFTP server lacks filesystem permissions

**Solution:** Ensure TFTP user has write access to target directory

## Best Practices

1. **Start restrictive** - Begin with `enabled = false` and add patterns incrementally
2. **Use specific patterns** - Prefer `firmware/*.bin` over `**/*.bin`
3. **Monitor audit logs** - Review write attempts regularly
4. **Test configurations** - Verify patterns before production deployment
5. **Document patterns** - Maintain comments explaining each allowed pattern
6. **Regular reviews** - Audit allowed_patterns periodically
7. **Separate directories** - Use different root_dir for read vs write if possible

## Compliance Notes

This implementation supports the following compliance frameworks:

- **NIST 800-53:** AC-3, AC-6, AU-2, AU-3, CM-5, SI-7, SI-10
- **STIG:** V-222602 (access restrictions), V-222563 (audit records)
- **RFC 1350:** Full WRQ protocol compliance
- **RFC 2347:** TFTP Option Extension support

## References

- [RFC 1350 - The TFTP Protocol (Revision 2)](https://tools.ietf.org/html/rfc1350)
- [RFC 2347 - TFTP Option Extension](https://tools.ietf.org/html/rfc2347)
- [NIST 800-53 Security Controls](https://csrc.nist.gov/publications/detail/sp/800-53/rev-5/final)
- [Elastic Stack SIEM Integration](./elastic-setup.md)
