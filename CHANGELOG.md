# Changelog

All notable changes to this project will be documented in this file.

## [0.1.18] - 2026-07-19

### Fixed

- Audit log now shows plain IPv4 addresses instead of IPv4-mapped IPv6
  (`10.1.0.100:33721` instead of `[::ffff:10.1.0.100]:33721`)
- Audit log now shows relative file paths instead of absolute filesystem paths
  (`poly-8-6/site.cfg` instead of `/var/lib/usg-tftp/tftp/poly-8-6/site.cfg`)

## [0.1.17] - 2026-07-18

### Fixed

- TFTP audit events now visible in web UI: added file-based JSONL audit logger
  that writes to the shared data PVC (`{root_dir}/.audit/tftp-audit.jsonl`)
  instead of relying on the tracing log file (which was on a separate emptyDir)
- Web UI audit log also moved to shared data PVC for persistence across restarts

### Added

- `logging.audit_file` config option for explicit audit JSONL path
- `service.labels` Helm value for adding labels to the TFTP Service
  (needed for BGP advertisement selectors like `network: k3s-pod-cidrs`)

### Security

- Path validation now rejects hidden paths (segments starting with `.`)
  to protect the `.audit/` directory from TFTP client access

## [0.1.16] - 2026-07-18

### Fixed

- NETASCII streaming: spillover buffer now drains all full blocks per iteration
  (was growing unboundedly and sending oversized packets at EOF)
- Empty final DATA block now sent correctly in streaming path when windowsize > 1
  and file size is an exact multiple of block_size
- Block number wrapping: ACK and DATA handling uses wrapping-aware u16 arithmetic
  for transfers exceeding 65535 blocks
- NETASCII encoding: bare CR now correctly converts to CR+NUL per RFC 854
  (was CR+LF); CR+NUL decodes back to CR
- `send_with_retry` now actually retries with backoff (was single-shot)
- `tsize` pre-allocation capped to prevent OOM from malicious option values
- Web UI upload/delete handlers now enforce `allowed_patterns` and `allow_overwrite`
- Worker pool: fixed `default_windowsize` config, added recv/send fallbacks for
  non-Linux/FreeBSD platforms
- Path validation: aligned to per-segment `..` check (substring match was
  rejecting valid filenames containing `..` like `backup..2024.bin`)
- `server.rs`: fixed missing closing braces in WRQ and ACK handlers that
  prevented compilation

### Changed

- Worker pool now uses `path_security::validate_and_resolve_path` instead of
  the stub `TftpServer::validate_and_resolve_path`

## [0.1.15] - 2026-07-18

### Fixed

- Release workflow: replaced `softprops/action-gh-release` with `gh` CLI to
  fix failure on GitHub's immutable releases

## [0.1.14] - 2026-07-18

### Added

- PROXY protocol v1 (text) and v2 (binary) header parsing for real client IP
  preservation behind Traefik IngressRouteTCP with TLS passthrough
- `web.proxy_protocol` configuration option (default: `false`)
- Helm values: `webUI.proxyProtocol` with automatic `proxyProtocol.version: 2`
  on IngressRouteTCP when enabled
- Custom hyper-util serve loop for proxy-protocol-enabled connections with
  ConnectInfo injection
- TLS-ALPN-01 ACME challenge support through the custom TLS path

### Security

- 5-second timeout on PROXY header reads (slowloris protection)
- 10-second TLS handshake timeout
- Connections with invalid/timed-out PROXY headers are dropped immediately
  (stream integrity guarantee)
- Only PROXY command (0x01) accepted; all reserved command values rejected
- Address payload capped at 512 bytes to prevent unbounded allocation
- When proxy_protocol is enabled, X-Forwarded-For is ignored (prevents
  spoofing in TLS-passthrough deployments)

### Changed

- Accept loop uses escalating backoff (50ms-1s) with error-level logging
  after 10 consecutive failures
- `load_certs()` now logs warnings for skipped PEM entries instead of
  silently dropping them
- `serve_http()` extracted as a generic function for type-safe TLS and
  plain-TCP connection handling

## [0.1.13] - 2026-07-16

### Added

- Client IP tracking in web UI audit events

## [0.1.12] - 2026-07-15

### Added

- Audit trail for web UI file operations

## [0.1.11] - 2026-07-14

### Added

- Helm chart and multi-site deployment guide
- GitHub CI/CD workflows and Containerfile
