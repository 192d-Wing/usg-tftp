use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::Path;
use tracing::{Level, event};

/// Security audit event types for SIEM integration
///
/// NIST 800-53 Controls:
/// - AU-2: Audit Events (comprehensive event catalog)
/// - AU-3: Content of Audit Records (structured event data)
/// - AU-6: Audit Review, Analysis, and Reporting (SIEM integration)
/// - AU-12: Audit Generation (automatic event generation)
///
/// STIG V-222563: Applications must produce audit records
/// STIG V-222564: Applications must protect audit information
/// STIG V-222565: Applications must alert on audit processing failures
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum AuditEvent {
    /// Server startup event
    ServerStarted {
        #[serde(flatten)]
        common: CommonFields,
        bind_addr: String,
        root_dir: String,
        multicast_enabled: bool,
    },

    /// Server shutdown event
    ServerShutdown {
        #[serde(flatten)]
        common: CommonFields,
        reason: String,
    },

    /// Client connection initiated
    ConnectionInitiated {
        #[serde(flatten)]
        common: CommonFields,
        client_addr: String,
        opcode: String,
    },

    /// File read request received
    ReadRequest {
        #[serde(flatten)]
        common: CommonFields,
        client_addr: String,
        filename: String,
        mode: String,
        options: serde_json::Value,
    },

    /// File read request denied
    ReadDenied {
        #[serde(flatten)]
        common: CommonFields,
        client_addr: String,
        filename: String,
        reason: String,
    },

    /// File transfer started
    TransferStarted {
        #[serde(flatten)]
        common: CommonFields,
        client_addr: String,
        filename: String,
        file_size: u64,
        mode: String,
        block_size: usize,
    },

    /// File transfer completed successfully
    TransferCompleted {
        #[serde(flatten)]
        common: CommonFields,
        client_addr: String,
        filename: String,
        bytes_transferred: u64,
        blocks_sent: u16,
        duration_ms: u64,
        /// Transfer throughput in bytes per second
        throughput_bps: u64,
        /// Average block transfer time in milliseconds
        avg_block_time_ms: f64,
    },

    /// File transfer failed
    TransferFailed {
        #[serde(flatten)]
        common: CommonFields,
        client_addr: String,
        filename: String,
        error: String,
        blocks_sent: u16,
    },

    /// Write request received
    WriteRequest {
        #[serde(flatten)]
        common: CommonFields,
        client_addr: String,
        filename: String,
        mode: String,
        options: serde_json::Value,
    },

    /// Write request denied
    WriteRequestDenied {
        #[serde(flatten)]
        common: CommonFields,
        client_addr: String,
        filename: String,
        reason: String,
    },

    /// Write operation started
    WriteStarted {
        #[serde(flatten)]
        common: CommonFields,
        client_addr: String,
        filename: String,
        mode: String,
        block_size: usize,
    },

    /// Write operation completed
    WriteCompleted {
        #[serde(flatten)]
        common: CommonFields,
        client_addr: String,
        filename: String,
        bytes_received: u64,
        blocks_received: u16,
        duration_ms: u64,
        /// Transfer throughput in bytes per second
        throughput_bps: u64,
        /// Average time per block in milliseconds
        avg_block_time_ms: f64,
        file_created: bool,
    },

    /// Write operation failed
    WriteFailed {
        #[serde(flatten)]
        common: CommonFields,
        client_addr: String,
        filename: String,
        error: String,
        blocks_received: u16,
    },

    /// Path traversal attempt detected
    PathTraversalAttempt {
        #[serde(flatten)]
        common: CommonFields,
        client_addr: String,
        requested_path: String,
        violation_type: String,
    },

    /// Access violation detected
    AccessViolation {
        #[serde(flatten)]
        common: CommonFields,
        client_addr: String,
        resource: String,
        violation: String,
    },

    /// File size limit exceeded
    FileSizeLimitExceeded {
        #[serde(flatten)]
        common: CommonFields,
        client_addr: String,
        filename: String,
        file_size: u64,
        max_allowed: u64,
    },

    /// Invalid protocol operation
    ProtocolViolation {
        #[serde(flatten)]
        common: CommonFields,
        client_addr: String,
        violation: String,
    },

    /// Multicast session events
    MulticastSessionCreated {
        #[serde(flatten)]
        common: CommonFields,
        session_id: String,
        filename: String,
        multicast_addr: String,
        multicast_port: u16,
    },

    /// Client joined multicast session
    MulticastClientJoined {
        #[serde(flatten)]
        common: CommonFields,
        session_id: String,
        client_addr: String,
        is_master: bool,
        total_clients: usize,
    },

    /// Client removed from multicast session
    MulticastClientRemoved {
        #[serde(flatten)]
        common: CommonFields,
        session_id: String,
        client_addr: String,
        reason: String,
        remaining_clients: usize,
    },

    /// Multicast session completed
    MulticastSessionCompleted {
        #[serde(flatten)]
        common: CommonFields,
        session_id: String,
        total_blocks: u16,
        total_clients: usize,
        duration_ms: u64,
        /// Total bytes transferred in the session
        bytes_transferred: u64,
        /// Number of retransmission cycles required
        retransmission_count: usize,
    },

    /// Rate limiting or DoS protection triggered
    RateLimitTriggered {
        #[serde(flatten)]
        common: CommonFields,
        client_addr: String,
        reason: String,
    },

    /// Configuration loaded/changed
    ConfigurationLoaded {
        #[serde(flatten)]
        common: CommonFields,
        config_file: String,
    },

    /// Configuration validation error
    ConfigurationError {
        #[serde(flatten)]
        common: CommonFields,
        config_file: String,
        error: String,
    },

    /// Symlink access attempt
    SymlinkAccessDenied {
        #[serde(flatten)]
        common: CommonFields,
        client_addr: String,
        requested_path: String,
    },

    /// Authentication event (reserved for future use)
    AuthenticationAttempt {
        #[serde(flatten)]
        common: CommonFields,
        client_addr: String,
        username: Option<String>,
        success: bool,
    },

    /// Resource exhaustion warning
    ResourceExhaustion {
        #[serde(flatten)]
        common: CommonFields,
        resource_type: String,
        current_value: String,
        threshold: String,
    },
}

/// Common fields present in all audit events
///
/// NIST 800-53 AU-3: Content of Audit Records
/// - Date and time of the event
/// - Type of event
/// - Subject identity (when applicable)
/// - Outcome of the event
/// - Additional information as needed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommonFields {
    /// ISO 8601 timestamp
    pub timestamp: String,
    /// Hostname or system identifier
    pub hostname: String,
    /// Service name
    pub service: String,
    /// Severity level (info, warn, error)
    pub severity: String,
    /// Optional correlation ID for tracking related events
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

impl CommonFields {
    /// Create common fields with current timestamp
    pub fn new(severity: &str) -> Self {
        Self {
            timestamp: chrono::Utc::now().to_rfc3339(),
            hostname: hostname::get()
                .ok()
                .and_then(|h| h.into_string().ok())
                .unwrap_or_else(|| "unknown".to_string()),
            service: "snow-owl-tftp".to_string(),
            severity: severity.to_string(),
            correlation_id: None,
        }
    }

    /// Create common fields with correlation ID
    pub fn with_correlation(severity: &str, correlation_id: String) -> Self {
        let mut fields = Self::new(severity);
        fields.correlation_id = Some(correlation_id);
        fields
    }
}

impl AuditEvent {
    /// Log this audit event using structured tracing
    ///
    /// NIST 800-53 Controls:
    /// - AU-12: Audit Generation (automated event generation)
    /// - AU-3: Content of Audit Records (structured data)
    pub fn log(&self) {
        let severity = self.severity();
        let json = serde_json::to_string(self).unwrap_or_else(|_| {
            format!(
                "{{\"error\": \"Failed to serialize audit event: {:?}\"}}",
                self
            )
        });

        match severity.as_str() {
            "error" => event!(Level::ERROR, audit_event = %json),
            "warn" => event!(Level::WARN, audit_event = %json),
            _ => event!(Level::INFO, audit_event = %json),
        }
    }

    /// Get the severity level of this event
    fn severity(&self) -> String {
        match self {
            AuditEvent::ServerStarted { common, .. }
            | AuditEvent::ServerShutdown { common, .. }
            | AuditEvent::ConnectionInitiated { common, .. }
            | AuditEvent::ReadRequest { common, .. }
            | AuditEvent::WriteRequest { common, .. }
            | AuditEvent::TransferStarted { common, .. }
            | AuditEvent::TransferCompleted { common, .. }
            | AuditEvent::WriteStarted { common, .. }
            | AuditEvent::WriteCompleted { common, .. }
            | AuditEvent::MulticastSessionCreated { common, .. }
            | AuditEvent::MulticastClientJoined { common, .. }
            | AuditEvent::MulticastSessionCompleted { common, .. }
            | AuditEvent::ConfigurationLoaded { common, .. } => common.severity.clone(),

            AuditEvent::ReadDenied { common, .. }
            | AuditEvent::WriteRequestDenied { common, .. }
            | AuditEvent::MulticastClientRemoved { common, .. } => common.severity.clone(),

            AuditEvent::PathTraversalAttempt { common, .. }
            | AuditEvent::AccessViolation { common, .. }
            | AuditEvent::FileSizeLimitExceeded { common, .. }
            | AuditEvent::ProtocolViolation { common, .. }
            | AuditEvent::TransferFailed { common, .. }
            | AuditEvent::WriteFailed { common, .. }
            | AuditEvent::RateLimitTriggered { common, .. }
            | AuditEvent::ConfigurationError { common, .. }
            | AuditEvent::SymlinkAccessDenied { common, .. }
            | AuditEvent::AuthenticationAttempt { common, .. }
            | AuditEvent::ResourceExhaustion { common, .. } => common.severity.clone(),
        }
    }
}

/// Audit logger for TFTP operations
///
/// NIST 800-53 Controls:
/// - AU-2: Audit Events
/// - AU-3: Content of Audit Records
/// - AU-9: Protection of Audit Information
pub struct AuditLogger;

impl AuditLogger {
    /// Log server startup
    pub fn server_started(bind_addr: &str, root_dir: &str, multicast_enabled: bool) {
        AuditEvent::ServerStarted {
            common: CommonFields::new("info"),
            bind_addr: bind_addr.to_string(),
            root_dir: root_dir.to_string(),
            multicast_enabled,
        }
        .log();
    }

    /// Generate a correlation ID for tracking related transfer events
    /// Format: <timestamp>-<client_addr>-<filename_hash>
    pub fn generate_correlation_id(client_addr: SocketAddr, filename: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        filename.hash(&mut hasher);
        let hash = hasher.finish();

        format!(
            "{:x}-{}-{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            client_addr.to_string().replace(':', "-"),
            hash
        )
    }

    /// Log read request with correlation ID
    pub fn read_request_with_correlation(
        client_addr: SocketAddr,
        filename: &str,
        mode: &str,
        options: serde_json::Value,
        correlation_id: &str,
    ) {
        AuditEvent::ReadRequest {
            common: CommonFields::with_correlation("info", correlation_id.to_string()),
            client_addr: client_addr.to_string(),
            filename: filename.to_string(),
            mode: mode.to_string(),
            options,
        }
        .log();
    }

    /// Log read request (without correlation ID - for backward compatibility)
    pub fn read_request(
        client_addr: SocketAddr,
        filename: &str,
        mode: &str,
        options: serde_json::Value,
    ) {
        AuditEvent::ReadRequest {
            common: CommonFields::new("info"),
            client_addr: client_addr.to_string(),
            filename: filename.to_string(),
            mode: mode.to_string(),
            options,
        }
        .log();
    }

    /// Log read denied
    pub fn read_denied(client_addr: SocketAddr, filename: &str, reason: &str) {
        AuditEvent::ReadDenied {
            common: CommonFields::new("warn"),
            client_addr: client_addr.to_string(),
            filename: filename.to_string(),
            reason: reason.to_string(),
        }
        .log();
    }

    /// Log transfer started with correlation ID
    pub fn transfer_started_with_correlation(
        client_addr: SocketAddr,
        filename: &str,
        file_size: u64,
        mode: &str,
        block_size: usize,
        correlation_id: &str,
    ) {
        AuditEvent::TransferStarted {
            common: CommonFields::with_correlation("info", correlation_id.to_string()),
            client_addr: client_addr.to_string(),
            filename: filename.to_string(),
            file_size,
            mode: mode.to_string(),
            block_size,
        }
        .log();
    }

    /// Log transfer started (without correlation ID - for backward compatibility)
    pub fn transfer_started(
        client_addr: SocketAddr,
        filename: &str,
        file_size: u64,
        mode: &str,
        block_size: usize,
    ) {
        AuditEvent::TransferStarted {
            common: CommonFields::new("info"),
            client_addr: client_addr.to_string(),
            filename: filename.to_string(),
            file_size,
            mode: mode.to_string(),
            block_size,
        }
        .log();
    }

    /// Log transfer completed with correlation ID
    pub fn transfer_completed_with_correlation(
        client_addr: SocketAddr,
        filename: &str,
        bytes_transferred: u64,
        blocks_sent: u16,
        duration_ms: u64,
        correlation_id: &str,
    ) {
        // Calculate performance metrics
        let throughput_bps = if duration_ms > 0 {
            (bytes_transferred * 1000) / duration_ms
        } else {
            0
        };

        let avg_block_time_ms = if blocks_sent > 0 && duration_ms > 0 {
            duration_ms as f64 / blocks_sent as f64
        } else {
            0.0
        };

        AuditEvent::TransferCompleted {
            common: CommonFields::with_correlation("info", correlation_id.to_string()),
            client_addr: client_addr.to_string(),
            filename: filename.to_string(),
            bytes_transferred,
            blocks_sent,
            duration_ms,
            throughput_bps,
            avg_block_time_ms,
        }
        .log();
    }

    /// Log transfer completed (without correlation ID - for backward compatibility)
    pub fn transfer_completed(
        client_addr: SocketAddr,
        filename: &str,
        bytes_transferred: u64,
        blocks_sent: u16,
        duration_ms: u64,
    ) {
        // Calculate performance metrics
        let throughput_bps = if duration_ms > 0 {
            (bytes_transferred * 1000) / duration_ms
        } else {
            0
        };

        let avg_block_time_ms = if blocks_sent > 0 && duration_ms > 0 {
            duration_ms as f64 / blocks_sent as f64
        } else {
            0.0
        };

        AuditEvent::TransferCompleted {
            common: CommonFields::new("info"),
            client_addr: client_addr.to_string(),
            filename: filename.to_string(),
            bytes_transferred,
            blocks_sent,
            duration_ms,
            throughput_bps,
            avg_block_time_ms,
        }
        .log();
    }

    /// Log transfer failed
    pub fn transfer_failed(client_addr: SocketAddr, filename: &str, error: &str, blocks_sent: u16) {
        AuditEvent::TransferFailed {
            common: CommonFields::new("error"),
            client_addr: client_addr.to_string(),
            filename: filename.to_string(),
            error: error.to_string(),
            blocks_sent,
        }
        .log();
    }

    /// Log write request denied
    /// Log write request
    pub fn write_request(
        client_addr: SocketAddr,
        filename: &str,
        mode: &str,
        options: serde_json::Value,
    ) {
        AuditEvent::WriteRequest {
            common: CommonFields::new("info"),
            client_addr: client_addr.to_string(),
            filename: filename.to_string(),
            mode: mode.to_string(),
            options,
        }
        .log();
    }

    /// Log write request denied
    pub fn write_request_denied(client_addr: SocketAddr, filename: &str, reason: &str) {
        AuditEvent::WriteRequestDenied {
            common: CommonFields::new("warn"),
            client_addr: client_addr.to_string(),
            filename: filename.to_string(),
            reason: reason.to_string(),
        }
        .log();
    }

    /// Log write started
    pub fn write_started(client_addr: SocketAddr, filename: &str, mode: &str, block_size: usize) {
        AuditEvent::WriteStarted {
            common: CommonFields::new("info"),
            client_addr: client_addr.to_string(),
            filename: filename.to_string(),
            mode: mode.to_string(),
            block_size,
        }
        .log();
    }

    /// Log write completed
    pub fn write_completed(
        client_addr: SocketAddr,
        filename: &str,
        bytes_received: u64,
        blocks_received: u16,
        duration_ms: u64,
        file_created: bool,
    ) {
        let throughput_bps = if duration_ms > 0 {
            (bytes_received * 1000) / duration_ms
        } else {
            0
        };

        let avg_block_time_ms = if blocks_received > 0 && duration_ms > 0 {
            duration_ms as f64 / blocks_received as f64
        } else {
            0.0
        };

        AuditEvent::WriteCompleted {
            common: CommonFields::new("info"),
            client_addr: client_addr.to_string(),
            filename: filename.to_string(),
            bytes_received,
            blocks_received,
            duration_ms,
            throughput_bps,
            avg_block_time_ms,
            file_created,
        }
        .log();
    }

    /// Log write failed
    pub fn write_failed(
        client_addr: SocketAddr,
        filename: &str,
        error: &str,
        blocks_received: u16,
    ) {
        AuditEvent::WriteFailed {
            common: CommonFields::new("error"),
            client_addr: client_addr.to_string(),
            filename: filename.to_string(),
            error: error.to_string(),
            blocks_received,
        }
        .log();
    }

    /// Log path traversal attempt
    pub fn path_traversal_attempt(
        client_addr: SocketAddr,
        requested_path: &str,
        violation_type: &str,
    ) {
        AuditEvent::PathTraversalAttempt {
            common: CommonFields::new("error"),
            client_addr: client_addr.to_string(),
            requested_path: requested_path.to_string(),
            violation_type: violation_type.to_string(),
        }
        .log();
    }

    /// Log access violation
    pub fn access_violation(client_addr: SocketAddr, resource: &str, violation: &str) {
        AuditEvent::AccessViolation {
            common: CommonFields::new("error"),
            client_addr: client_addr.to_string(),
            resource: resource.to_string(),
            violation: violation.to_string(),
        }
        .log();
    }

    /// Log file size limit exceeded
    pub fn file_size_limit_exceeded(
        client_addr: SocketAddr,
        filename: &str,
        file_size: u64,
        max_allowed: u64,
    ) {
        AuditEvent::FileSizeLimitExceeded {
            common: CommonFields::new("error"),
            client_addr: client_addr.to_string(),
            filename: filename.to_string(),
            file_size,
            max_allowed,
        }
        .log();
    }

    /// Log protocol violation
    pub fn protocol_violation(client_addr: SocketAddr, violation: &str) {
        AuditEvent::ProtocolViolation {
            common: CommonFields::new("error"),
            client_addr: client_addr.to_string(),
            violation: violation.to_string(),
        }
        .log();
    }

    /// Log multicast session created
    pub fn multicast_session_created(
        session_id: &str,
        filename: &str,
        multicast_addr: &str,
        multicast_port: u16,
    ) {
        AuditEvent::MulticastSessionCreated {
            common: CommonFields::new("info"),
            session_id: session_id.to_string(),
            filename: filename.to_string(),
            multicast_addr: multicast_addr.to_string(),
            multicast_port,
        }
        .log();
    }

    /// Log multicast client joined
    pub fn multicast_client_joined(
        session_id: &str,
        client_addr: SocketAddr,
        is_master: bool,
        total_clients: usize,
    ) {
        AuditEvent::MulticastClientJoined {
            common: CommonFields::new("info"),
            session_id: session_id.to_string(),
            client_addr: client_addr.to_string(),
            is_master,
            total_clients,
        }
        .log();
    }

    /// Log multicast client removed
    pub fn multicast_client_removed(
        session_id: &str,
        client_addr: SocketAddr,
        reason: &str,
        remaining_clients: usize,
    ) {
        AuditEvent::MulticastClientRemoved {
            common: CommonFields::new("warn"),
            session_id: session_id.to_string(),
            client_addr: client_addr.to_string(),
            reason: reason.to_string(),
            remaining_clients,
        }
        .log();
    }

    /// Log symlink access denied
    pub fn symlink_access_denied(client_addr: SocketAddr, requested_path: &str) {
        AuditEvent::SymlinkAccessDenied {
            common: CommonFields::new("error"),
            client_addr: client_addr.to_string(),
            requested_path: requested_path.to_string(),
        }
        .log();
    }

    /// Log configuration loaded
    pub fn configuration_loaded(config_file: &Path) {
        AuditEvent::ConfigurationLoaded {
            common: CommonFields::new("info"),
            config_file: config_file.display().to_string(),
        }
        .log();
    }

    /// Log configuration error
    pub fn configuration_error(config_file: &Path, error: &str) {
        AuditEvent::ConfigurationError {
            common: CommonFields::new("error"),
            config_file: config_file.display().to_string(),
            error: error.to_string(),
        }
        .log();
    }
}
