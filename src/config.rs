use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;

use crate::error::{Result, TftpError};

/// Write operation configuration for TFTP
///
/// NIST 800-53 Controls:
/// - AC-3: Access Enforcement (restrict write access)
/// - AC-6: Least Privilege (minimal write permissions)
/// - CM-5: Access Restrictions for Change (control file modifications)
///
/// STIG V-222602: Applications must enforce access restrictions
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct WriteConfig {
    /// Enable write operations (disabled by default for security)
    pub enabled: bool,

    /// Allow overwriting existing files
    /// When false, returns "File already exists" error per RFC 1350
    pub allow_overwrite: bool,

    /// List of glob patterns that are allowed to be written
    /// Examples: ["*.txt", "configs/*.cfg", "firmware/device-*.bin"]
    /// Empty list means no writes are allowed
    pub allowed_patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TftpConfig {
    pub root_dir: PathBuf,
    pub bind_addr: SocketAddr,
    pub multicast: MulticastConfig,
    pub logging: LoggingConfig,
    pub write_config: WriteConfig,
    pub performance: PerformanceConfig,
    /// Maximum file size in bytes that can be served (default: 100MB)
    /// Set to 0 for unlimited (not recommended for security)
    pub max_file_size_bytes: u64,
}

impl Default for TftpConfig {
    fn default() -> Self {
        Self {
            root_dir: PathBuf::from("/var/lib/snow-owl/tftp"),
            bind_addr: SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), 69),
            multicast: MulticastConfig::default(),
            logging: LoggingConfig::default(),
            write_config: WriteConfig::default(),
            performance: PerformanceConfig::default(),
            max_file_size_bytes: 104_857_600, // 100 MB default
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
    pub format: LogFormat,
    pub file: Option<PathBuf>,
    /// Enable structured audit logging for SIEM integration
    /// When enabled, all security-relevant events are logged as structured JSON
    pub audit_enabled: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            format: LogFormat::Json,
            file: Some(PathBuf::from("/var/log/snow-owl/tftp-audit.json")),
            audit_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Plain text logging for human readability
    Text,
    /// JSON structured logging for SIEM integration
    /// All log entries are formatted as JSON for easy parsing by log aggregators
    Json,
}

/// Multicast TFTP configuration (RFC 2090)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MulticastConfig {
    pub enabled: bool,
    pub multicast_addr: IpAddr,
    pub multicast_ip_version: MulticastIpVersion,
    pub multicast_port: u16,
    pub max_clients: usize,
    pub master_timeout_secs: u64,
    pub retransmit_timeout_secs: u64,
}

impl Default for MulticastConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            multicast_addr: default_multicast_addr_v6(),
            multicast_ip_version: MulticastIpVersion::V6,
            multicast_port: default_multicast_port(),
            max_clients: default_max_clients(),
            master_timeout_secs: default_master_timeout(),
            retransmit_timeout_secs: default_retransmit_timeout(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MulticastIpVersion {
    V4,
    V6,
}

fn default_multicast_addr_v6() -> IpAddr {
    IpAddr::V6(Ipv6Addr::new(0xff12, 0, 0, 0, 0, 0, 0x8000, 0x0001))
}

#[allow(dead_code)]
fn default_multicast_addr_v4() -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(224, 0, 1, 1))
}

pub(crate) fn default_multicast_addr_for_version(version: MulticastIpVersion) -> IpAddr {
    match version {
        MulticastIpVersion::V4 => default_multicast_addr_v4(),
        MulticastIpVersion::V6 => default_multicast_addr_v6(),
    }
}

pub(crate) fn load_config(path: &std::path::Path) -> Result<TftpConfig> {
    let contents = std::fs::read_to_string(path)?;
    let config: TftpConfig = toml::from_str(&contents)
        .map_err(|e| TftpError::Tftp(format!("Invalid config file {}: {}", path.display(), e)))?;
    Ok(config)
}

pub(crate) fn write_default_config(path: &std::path::Path) -> Result<()> {
    write_config(path, &TftpConfig::default())
}

pub(crate) fn write_config(path: &std::path::Path, config: &TftpConfig) -> Result<()> {
    let contents = toml::to_string_pretty(config)
        .map_err(|e| TftpError::Tftp(format!("Failed to serialize default config: {}", e)))?;
    std::fs::write(path, contents)?;
    Ok(())
}

/// Validate TFTP configuration for security and correctness
///
/// NIST 800-53 Controls:
/// - CM-6: Configuration Settings (validate all configuration parameters)
/// - AC-3: Access Enforcement (validate directory permissions)
/// - SC-7: Boundary Protection (validate network bindings)
/// - SC-5: Denial of Service Protection (validate resource limits)
///
/// STIG V-222564: Applications must protect configuration data
/// STIG V-222566: Applications must validate configuration parameters
/// STIG V-222602: Applications must enforce access restrictions
pub(crate) fn validate_config(config: &TftpConfig, validate_bind: bool) -> Result<()> {
    // NIST CM-6: Validate root directory is absolute path
    // STIG V-222566: Configuration parameter validation
    if !config.root_dir.is_absolute() {
        return Err(TftpError::Tftp(
            "root_dir must be an absolute path".to_string(),
        ));
    }

    // NIST AC-3: Validate directory exists and is accessible
    // STIG V-222602: Enforce access restrictions
    match std::fs::metadata(&config.root_dir) {
        Ok(meta) => {
            if !meta.is_dir() {
                return Err(TftpError::Tftp("root_dir must be a directory".to_string()));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(TftpError::Tftp(
                "root_dir does not exist; create it or adjust config".to_string(),
            ));
        }
        Err(e) => return Err(TftpError::Io(e)),
    }

    // NIST AC-3: Validate directory is readable
    if let Err(e) = std::fs::read_dir(&config.root_dir) {
        return Err(TftpError::Tftp(format!("root_dir is not readable: {}", e)));
    }

    if config.bind_addr.port() == 0 {
        return Err(TftpError::Tftp(
            "bind_addr port must be non-zero".to_string(),
        ));
    }

    if validate_bind && let Err(e) = std::net::UdpSocket::bind(config.bind_addr) {
        return Err(TftpError::Tftp(format!(
            "bind_addr is not available: {}",
            e
        )));
    }

    if !(1024..=65535).contains(&config.multicast.multicast_port) {
        return Err(TftpError::Tftp(
            "multicast_port must be in range 1024-65535".to_string(),
        ));
    }

    if let Some(ref log_file) = config.logging.file {
        let parent = log_file.parent().ok_or_else(|| {
            TftpError::Tftp("logging.file must include a parent directory".to_string())
        })?;
        match std::fs::metadata(parent) {
            Ok(meta) => {
                if !meta.is_dir() {
                    return Err(TftpError::Tftp(
                        "logging.file parent must be a directory".to_string(),
                    ));
                }
            }
            Err(e) => return Err(TftpError::Tftp(format!("logging.file parent error: {}", e))),
        }
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file)
            .map_err(|e| TftpError::Tftp(format!("logging.file not writable: {}", e)))?;
    }

    validate_multicast_config(&config.multicast)?;
    validate_write_config(&config.write_config)?;
    Ok(())
}

pub(crate) fn validate_multicast_config(config: &MulticastConfig) -> Result<()> {
    let version_matches = matches!(
        (config.multicast_ip_version, config.multicast_addr),
        (MulticastIpVersion::V4, IpAddr::V4(_)) | (MulticastIpVersion::V6, IpAddr::V6(_))
    );

    if !version_matches {
        return Err(TftpError::Tftp(
            "Multicast address does not match multicast IP version".to_string(),
        ));
    }

    Ok(())
}

pub(crate) fn validate_write_config(config: &WriteConfig) -> Result<()> {
    // NIST AC-3: If writes are enabled, require at least one allowed pattern
    // STIG V-222602: Enforce explicit access restrictions
    if config.enabled && config.allowed_patterns.is_empty() {
        return Err(TftpError::Tftp(
            "Write operations enabled but no allowed_patterns specified. \
            Add patterns to allowed_patterns or disable writes."
                .to_string(),
        ));
    }

    // Validate patterns are not overly permissive
    // NIST AC-6: Least Privilege
    for pattern in &config.allowed_patterns {
        if pattern.trim().is_empty() {
            return Err(TftpError::Tftp(
                "Write allowed_patterns cannot contain empty patterns".to_string(),
            ));
        }

        // Warn about overly permissive patterns
        if pattern == "*" || pattern == "**" || pattern == "**/*" {
            return Err(TftpError::Tftp(format!(
                "Write pattern '{}' is too permissive. Use specific patterns like '*.txt' or 'subdir/*.cfg'",
                pattern
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::io::Result<PathBuf> {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "snow_owl_tftp_test_{}_{}",
            name,
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    #[test]
    fn parses_minimal_toml() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root_dir = temp_dir("parse")?;
        let log_dir = temp_dir("parse_log")?;
        let toml = format!(
            r#"
root_dir = "{}"
bind_addr = "127.0.0.1:6969"

[logging]
file = "{}/tftp.log"
"#,
            root_dir.display(),
            log_dir.display()
        );
        let config: TftpConfig = toml::from_str(&toml)?;
        validate_config(&config, false)?;
        Ok(())
    }

    #[test]
    fn rejects_non_absolute_root_dir() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let log_dir = temp_dir("non_abs_log")?;
        let config = TftpConfig {
            root_dir: PathBuf::from("relative/path"),
            logging: LoggingConfig {
                file: Some(log_dir.join("tftp.log")),
                ..Default::default()
            },
            ..Default::default()
        };
        match validate_config(&config, false) {
            Ok(()) => return Err("expected error for relative root_dir".into()),
            Err(err) => {
                assert!(format!("{err}").contains("root_dir must be an absolute path"));
            }
        }
        Ok(())
    }

    #[test]
    fn rejects_unreadable_root_dir() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let log_dir = temp_dir("unreadable_log")?;
        let config = TftpConfig {
            root_dir: PathBuf::from("/nonexistent/snow-owl-tftp"),
            logging: LoggingConfig {
                file: Some(log_dir.join("tftp.log")),
                ..Default::default()
            },
            ..Default::default()
        };
        match validate_config(&config, false) {
            Ok(()) => return Err("expected error for missing root_dir".into()),
            Err(err) => {
                assert!(format!("{err}").contains("root_dir does not exist"));
            }
        }
        Ok(())
    }

    #[test]
    fn rejects_zero_bind_port() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = TftpConfig::default();
        config.root_dir = temp_dir("bind")?;
        config.bind_addr = "127.0.0.1:0".parse()?;
        match validate_config(&config, false) {
            Ok(()) => return Err("expected error for zero bind port".into()),
            Err(err) => {
                assert!(format!("{err}").contains("bind_addr port must be non-zero"));
            }
        }
        Ok(())
    }

    #[test]
    fn rejects_multicast_port_out_of_range() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
        let mut config = TftpConfig::default();
        config.root_dir = temp_dir("mcast-port")?;
        config.multicast.multicast_port = 100;
        match validate_config(&config, false) {
            Ok(()) => return Err("expected error for multicast_port range".into()),
            Err(err) => {
                assert!(format!("{err}").contains("multicast_port must be in range"));
            }
        }
        Ok(())
    }

    #[test]
    fn rejects_mismatched_multicast_version() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
        let log_dir = temp_dir("mcast_ver_log")?;
        let mut config = TftpConfig::default();
        config.root_dir = temp_dir("mcast-ver")?;
        config.logging.file = Some(log_dir.join("tftp.log"));
        config.multicast.multicast_ip_version = MulticastIpVersion::V4;
        config.multicast.multicast_addr =
            IpAddr::V6(Ipv6Addr::new(0xff12, 0, 0, 0, 0, 0, 0x8000, 0x0001));
        match validate_config(&config, false) {
            Ok(()) => return Err("expected error for multicast version mismatch".into()),
            Err(err) => {
                assert!(format!("{err}").contains("Multicast address does not match"));
            }
        }
        Ok(())
    }

    #[test]
    fn rejects_logging_file_with_missing_parent()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut config = TftpConfig::default();
        config.root_dir = temp_dir("logfile")?;
        config.logging.file = Some(PathBuf::from("/nonexistent/snow-owl-tftp/log.txt"));
        match validate_config(&config, false) {
            Ok(()) => return Err("expected error for logging.file parent".into()),
            Err(err) => {
                assert!(format!("{err}").contains("logging.file parent error"));
            }
        }
        Ok(())
    }

    #[test]
    fn validates_bind_addr_availability_on_free_port()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0")?;
        let port = socket.local_addr()?.port();
        drop(socket);

        let log_dir = temp_dir("bind_av_log")?;
        let mut config = TftpConfig::default();
        config.root_dir = temp_dir("bind-available")?;
        config.bind_addr = format!("127.0.0.1:{port}").parse()?;
        config.logging.file = Some(log_dir.join("tftp.log"));
        validate_config(&config, true)?;
        Ok(())
    }

    #[test]
    fn rejects_bind_addr_when_in_use() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0")?;
        let port = socket.local_addr()?.port();

        let mut config = TftpConfig::default();
        config.root_dir = temp_dir("bind-in-use")?;
        config.bind_addr = format!("127.0.0.1:{port}").parse()?;
        match validate_config(&config, true) {
            Ok(()) => return Err("expected error for bind_addr in use".into()),
            Err(err) => {
                assert!(format!("{err}").contains("bind_addr is not available"));
            }
        }
        Ok(())
    }

    #[test]
    fn rejects_writes_enabled_with_no_patterns()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let log_dir = temp_dir("write_no_pat_log")?;
        let mut config = TftpConfig::default();
        config.root_dir = temp_dir("write-no-patterns")?;
        config.logging.file = Some(log_dir.join("tftp.log"));
        config.write_config.enabled = true;
        config.write_config.allowed_patterns = vec![];
        match validate_config(&config, false) {
            Ok(()) => return Err("expected error for writes enabled without patterns".into()),
            Err(err) => {
                assert!(format!("{err}").contains("no allowed_patterns specified"));
            }
        }
        Ok(())
    }

    #[test]
    fn rejects_empty_pattern() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let log_dir = temp_dir("empty_pat_log")?;
        let mut config = TftpConfig::default();
        config.root_dir = temp_dir("empty-pattern")?;
        config.logging.file = Some(log_dir.join("tftp.log"));
        config.write_config.enabled = true;
        config.write_config.allowed_patterns = vec!["".to_string()];
        match validate_config(&config, false) {
            Ok(()) => return Err("expected error for empty pattern".into()),
            Err(err) => {
                assert!(format!("{err}").contains("cannot contain empty patterns"));
            }
        }
        Ok(())
    }

    #[test]
    fn rejects_overly_permissive_patterns() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let log_dir = temp_dir("permissive_log")?;

        for pattern in &["*", "**", "**/*"] {
            let mut config = TftpConfig::default();
            config.root_dir = temp_dir("permissive-pattern")?;
            config.logging.file = Some(log_dir.join("tftp.log"));
            config.write_config.enabled = true;
            config.write_config.allowed_patterns = vec![pattern.to_string()];
            match validate_config(&config, false) {
                Ok(()) => return Err(format!("expected error for pattern {}", pattern).into()),
                Err(err) => {
                    assert!(format!("{err}").contains("too permissive"));
                }
            }
        }
        Ok(())
    }

    #[test]
    fn accepts_valid_write_config() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let log_dir = temp_dir("valid_write_log")?;
        let mut config = TftpConfig::default();
        config.root_dir = temp_dir("valid-write")?;
        config.logging.file = Some(log_dir.join("tftp.log"));
        config.write_config.enabled = true;
        config.write_config.allow_overwrite = true;
        config.write_config.allowed_patterns = vec![
            "*.txt".to_string(),
            "configs/*.cfg".to_string(),
            "firmware/device-*.bin".to_string(),
        ];
        validate_config(&config, false)?;
        Ok(())
    }

    #[test]
    fn accepts_write_disabled() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let log_dir = temp_dir("write_disabled_log")?;
        let mut config = TftpConfig::default();
        config.root_dir = temp_dir("write-disabled")?;
        config.logging.file = Some(log_dir.join("tftp.log"));
        config.write_config.enabled = false;
        config.write_config.allowed_patterns = vec![]; // Empty is OK when disabled
        validate_config(&config, false)?;
        Ok(())
    }
}

fn default_multicast_port() -> u16 {
    1758
}

fn default_max_clients() -> usize {
    10
}

fn default_master_timeout() -> u64 {
    30
}

fn default_retransmit_timeout() -> u64 {
    5
}

/// Performance tuning configuration
///
/// These settings control performance optimizations for high-throughput scenarios
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PerformanceConfig {
    /// Default block size for transfers (bytes)
    /// RFC 1350 standard is 512, but larger sizes improve throughput
    /// Valid range: 512-65464
    pub default_block_size: usize,

    /// Default window size for RFC 7440 sliding window (blocks)
    /// RFC 7440: Valid range 1-65535, default 1 for RFC 1350 compatibility
    /// Higher values improve throughput on high-latency networks
    /// Recommended: 4-16 for typical networks, 32+ for high-latency links
    pub default_windowsize: usize,

    /// Buffer pool size for packet reuse
    /// Larger pools reduce allocations but use more memory
    pub buffer_pool_size: usize,

    /// Threshold for streaming vs buffered mode (bytes)
    /// Files smaller than this use full buffering for NETASCII conversion
    /// Larger files use streaming to minimize memory usage
    pub streaming_threshold: u64,

    /// Audit log sampling rate (0.0-1.0)
    /// 1.0 = log all events, 0.5 = log 50% of events
    /// Lower values reduce audit overhead for high-volume servers
    pub audit_sampling_rate: f64,

    /// Platform-specific performance optimizations (Linux/BSD)
    pub platform: PlatformPerformanceConfig,
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            default_block_size: 8192, // 8KB for better throughput
            default_windowsize: 1,    // RFC 1350 compatible (stop-and-wait)
            buffer_pool_size: 128,
            streaming_threshold: 1_048_576, // 1MB
            audit_sampling_rate: 1.0,       // Log everything by default
            platform: PlatformPerformanceConfig::default(),
        }
    }
}

/// Platform-specific performance optimizations for Linux/BSD systems
/// Phase 1: Socket tuning and file I/O hints
/// Phase 2: Zero-copy operations and batch syscalls
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PlatformPerformanceConfig {
    /// Socket-level optimizations
    pub socket: SocketConfig,

    /// File I/O optimization hints
    pub file_io: FileIoConfig,

    /// Batch packet operations (Phase 2)
    pub batch: BatchConfig,

    /// Zero-copy transfer optimizations (Phase 2)
    pub zero_copy: ZeroCopyConfig,
}

// Derived Default implementation

/// Socket-level performance configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SocketConfig {
    /// Receive buffer size in KB (SO_RCVBUF)
    /// Larger buffers reduce packet drops under high load
    /// Default: 2048 KB (2 MB)
    pub recv_buffer_kb: usize,

    /// Send buffer size in KB (SO_SNDBUF)
    /// Larger buffers improve burst handling
    /// Default: 2048 KB (2 MB)
    pub send_buffer_kb: usize,

    /// Enable SO_REUSEADDR for faster restarts
    /// Default: true
    pub reuse_address: bool,

    /// Enable SO_REUSEPORT for multi-process scaling (Linux 3.9+, BSD)
    /// Allows multiple processes to bind to same port
    /// Default: true on supported platforms
    pub reuse_port: bool,
}

impl Default for SocketConfig {
    fn default() -> Self {
        Self {
            recv_buffer_kb: 2048, // 2 MB
            send_buffer_kb: 2048, // 2 MB
            reuse_address: true,
            reuse_port: true,
        }
    }
}

/// File I/O optimization configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FileIoConfig {
    /// Use POSIX_FADV_SEQUENTIAL hint for sequential file reads
    /// Optimizes kernel read-ahead behavior
    /// Default: true
    pub use_sequential_hint: bool,

    /// Use POSIX_FADV_WILLNEED to prefetch file data
    /// Reduces I/O wait time
    /// Default: true
    pub use_willneed_hint: bool,

    /// Use POSIX_FADV_DONTNEED after transfer to free page cache
    /// Useful for large one-time transfers
    /// Default: false
    pub fadvise_dontneed_after: bool,
}

impl Default for FileIoConfig {
    fn default() -> Self {
        Self {
            use_sequential_hint: true,
            use_willneed_hint: true,
            fadvise_dontneed_after: false,
        }
    }
}

/// Batch packet operation configuration (Phase 2)
/// Uses sendmmsg()/recvmmsg() for reduced syscall overhead
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BatchConfig {
    /// Enable sendmmsg() for batch packet sending (Linux 3.0+, FreeBSD 11.0+)
    /// Reduces syscall overhead by 60-80% during concurrent transfers
    /// Default: true on supported platforms
    pub enable_sendmmsg: bool,

    /// Enable recvmmsg() for batch packet receiving (Linux 2.6.33+, FreeBSD 11.0+)
    /// Improves concurrent connection handling
    /// Default: true on supported platforms
    pub enable_recvmmsg: bool,

    /// Maximum number of packets to batch in a single syscall
    /// Higher values reduce overhead but increase latency
    /// Default: 32 packets
    pub max_batch_size: usize,

    /// Maximum time to wait for batching packets (microseconds)
    /// Lower values reduce latency, higher values improve batching efficiency
    /// Default: 100 microseconds
    pub batch_timeout_us: u64,

    /// Enable adaptive batching based on active client count
    /// When enabled, batch receiving is automatically disabled for low client counts
    /// to eliminate single-client latency regression
    /// Default: true
    pub enable_adaptive_batching: bool,

    /// Minimum number of active clients required to enable batch receiving
    /// When active clients < threshold, batch receiving is disabled for lower latency
    /// When active clients >= threshold, batch receiving is enabled for better throughput
    /// Default: 5 clients
    pub adaptive_batch_threshold: usize,
}

impl Default for BatchConfig {
    fn default() -> Self {
        // Enable by default on Linux and FreeBSD where supported
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let default_enabled = true;

        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        let default_enabled = false;

        Self {
            enable_sendmmsg: default_enabled,
            enable_recvmmsg: default_enabled,
            max_batch_size: 32,
            batch_timeout_us: 100,
            enable_adaptive_batching: true,
            adaptive_batch_threshold: 5,
        }
    }
}

/// Zero-copy transfer configuration (Phase 2)
/// Reduces CPU usage and memory bandwidth for large file transfers
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ZeroCopyConfig {
    /// Use sendfile() for zero-copy transfers (Linux only)
    /// Eliminates user-space buffer copies
    /// Default: true on Linux
    pub use_sendfile: bool,

    /// Minimum file size to use sendfile() (bytes)
    /// Smaller files may not benefit from zero-copy overhead
    /// Default: 65536 (64 KB)
    pub sendfile_threshold_bytes: u64,

    /// Use MSG_ZEROCOPY flag for send operations (Linux 4.14+)
    /// Reduces copies for large blocks, requires completion notification handling
    /// Default: false (experimental)
    pub use_msg_zerocopy: bool,

    /// Minimum block size to use MSG_ZEROCOPY (bytes)
    /// Only beneficial for larger blocks (>8KB)
    /// Default: 8192 (8 KB)
    pub msg_zerocopy_threshold_bytes: usize,
}

impl Default for ZeroCopyConfig {
    fn default() -> Self {
        #[cfg(target_os = "linux")]
        let default_sendfile = true;

        #[cfg(not(target_os = "linux"))]
        let default_sendfile = false;

        Self {
            use_sendfile: default_sendfile,
            sendfile_threshold_bytes: 65536,    // 64 KB
            use_msg_zerocopy: false,            // Experimental, requires completion handling
            msg_zerocopy_threshold_bytes: 8192, // 8 KB
        }
    }
}
