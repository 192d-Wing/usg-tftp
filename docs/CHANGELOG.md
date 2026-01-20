# Changelog - Snow-Owl TFTP Server

All notable changes to the Snow-Owl TFTP server will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- **PERFORMANCE:** Increased default `windowsize` from 1 to 8 for 8x throughput improvement
  - RFC 7440 sliding window now enabled by default
  - Fully backward compatible with legacy RFC 1350 clients (automatic fallback)
  - Improves performance on networks with RTT > 1ms
  - Minimal memory impact (~64KB per transfer vs ~8KB with windowsize=1)
  - See [WINDOWSIZE_PERFORMANCE_ANALYSIS.md](WINDOWSIZE_PERFORMANCE_ANALYSIS.md) for detailed analysis

### Added
- Comprehensive RFC 7440 windowsize test suite (32 tests)
  - Tests windowsize values 1-64
  - File sizes from 1KB to 512KB
  - Edge case coverage (single block, window boundaries)
  - 100% pass rate validation
- Performance analysis documentation
  - Throughput calculations for various RTT scenarios
  - Memory impact analysis
  - Network scenario recommendations
- Test infrastructure improvements
  - `windowsize-test.sh`: Bash-based functional tests (32 tests)
  - `windowsize-analyzer.py`: Python performance metrics tool
  - `run-all-tests.sh`: Master test runner with multiple options
  - Comprehensive test documentation

### Fixed
- Build system now uses `-p snow-owl-tftp` to avoid SFTP dependency issues
- Integration test suite reliability improvements
  - Binary path detection across multiple locations
  - Process cleanup to prevent port conflicts
  - ANSI color code handling in test output parsing
  - Rust PATH environment setup

## [0.1.0] - Initial Release

### Added
- RFC 1350 TFTP server implementation
- RFC 2347 Option Extension support
- RFC 2348 Blocksize Option (512-65464 bytes)
- RFC 2349 Timeout Interval and Transfer Size Options
- RFC 7440 Windowsize Option (1-65535 blocks)
- UDP socket with async I/O (Tokio)
- Configurable rate limiting
- File access control with pattern matching
- Path traversal protection
- Audit logging with sampling
- NETASCII and OCTET transfer modes
- Concurrent client handling
- Worker pool with buffer reuse
- Platform-specific optimizations (Linux SO_REUSEPORT)
- Integration test suite (10 tests)

### Security
- Path traversal prevention
- Write pattern validation
- Configurable file size limits
- Audit logging for security events

---

## Version History

| Version | Date | Key Changes |
|---------|------|-------------|
| Unreleased | 2026-01-19 | RFC 7440 windowsize optimization, comprehensive test suite |
| 0.1.0 | - | Initial implementation with RFC 1350/2347/2348/2349/7440 support |

---

## Migration Notes

### Upgrading to Windowsize=8 Default

**Impact:** Clients that support RFC 7440 will automatically use windowsize=8, improving throughput by 8x.

**Backward Compatibility:** ✅ Fully backward compatible
- Legacy RFC 1350 clients continue to work unchanged (windowsize=1)
- RFC 7440 clients negotiate optimal windowsize via OACK
- No configuration changes required

**Performance Expectations:**
- **LANs (RTT < 5ms):** 6.4 Mbps → 51.2 Mbps (8x improvement)
- **WANs (RTT 10-50ms):** 1.28 Mbps → 10.24 Mbps (8x improvement)
- **High-latency (RTT 100ms+):** Consider increasing to windowsize=16-32

**Configuration Override:**
To use a different default windowsize, update your `config.toml`:

```toml
[performance]
default_windowsize = 16  # Example: increase for WAN deployments
```

---

## References

- [RFC 1350](https://tools.ietf.org/html/rfc1350) - The TFTP Protocol (Revision 2)
- [RFC 2347](https://tools.ietf.org/html/rfc2347) - TFTP Option Extension
- [RFC 2348](https://tools.ietf.org/html/rfc2348) - TFTP Blocksize Option
- [RFC 2349](https://tools.ietf.org/html/rfc2349) - TFTP Timeout Interval and Transfer Size Options
- [RFC 7440](https://tools.ietf.org/html/rfc7440) - TFTP Windowsize Option

---

**Last Updated:** 2026-01-19
