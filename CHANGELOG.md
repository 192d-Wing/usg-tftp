# Changelog

All notable changes to this project will be documented in this file.

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
