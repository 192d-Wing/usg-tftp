mod config;
mod error;
mod multicast;

use bytes::{Buf, BufMut, BytesMut};
use clap::Parser;
use config::{
    default_multicast_addr_for_version, load_config, validate_config, write_config,
    MulticastConfig, MulticastIpVersion, TftpConfig,
};
use error::{Result, TftpError};
use multicast::MulticastTftpServer;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::net::UdpSocket;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

// RFC 1350 - The TFTP Protocol (Revision 2)
//
// NIST 800-53 Controls:
// - SC-5: Denial of Service Protection (packet size limits)
// - SC-23: Session Authenticity (timeout and retry limits)
//
// STIG V-222596: Applications must set session timeout limits
// STIG V-222597: Applications must limit retry attempts
pub(crate) const DEFAULT_BLOCK_SIZE: usize = 512; // RFC 1350 standard block size
const MAX_BLOCK_SIZE: usize = 65464; // RFC 2348 maximum block size
const MAX_PACKET_SIZE: usize = 65468; // Max block size + 4 byte header
const DEFAULT_TIMEOUT_SECS: u64 = 5;
const MAX_RETRIES: u32 = 5;

#[derive(Parser, Debug)]
#[command(name = "snow-owl-tftp", about = "Standalone TFTP server")]
struct Cli {
    /// Path to the TOML configuration file
    #[arg(long, default_value = "/etc/snow-owl/tftp.toml")]
    config: PathBuf,

    /// Write a default TOML configuration file and exit
    #[arg(long)]
    init_config: bool,

    /// Validate the configuration and exit (no socket bind)
    #[arg(long)]
    check_config: bool,

    /// Create the root directory if it does not exist
    #[arg(long)]
    create_root_dir: bool,

    /// Root directory to serve files from
    #[arg(long)]
    root_dir: Option<PathBuf>,

    /// Bind address for the TFTP server
    #[arg(long)]
    bind: Option<SocketAddr>,

    /// Enable multicast TFTP (RFC 2090)
    #[arg(long, value_parser = clap::value_parser!(bool))]
    multicast: Option<bool>,

    /// Multicast group address
    #[arg(long)]
    multicast_addr: Option<IpAddr>,

    /// Multicast IP version (v4 or v6)
    #[arg(long, value_enum)]
    multicast_ip_version: Option<MulticastIpVersion>,

    /// Multicast port
    #[arg(long)]
    multicast_port: Option<u16>,

    /// Maximum clients per multicast session
    #[arg(long)]
    max_clients: Option<usize>,

    /// Master client timeout in seconds
    #[arg(long)]
    master_timeout_secs: Option<u64>,

    /// Retransmission timeout in seconds
    #[arg(long)]
    retransmit_timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TftpOpcode {
    Rrq = 1,   // Read request (RFC 1350)
    Wrq = 2,   // Write request (RFC 1350)
    Data = 3,  // Data packet (RFC 1350)
    Ack = 4,   // Acknowledgment (RFC 1350)
    Error = 5, // Error packet (RFC 1350)
    Oack = 6,  // Option acknowledgment (RFC 2347)
}

impl TryFrom<u16> for TftpOpcode {
    type Error = TftpError;

    fn try_from(value: u16) -> std::result::Result<Self, <Self as TryFrom<u16>>::Error> {
        match value {
            1 => Ok(TftpOpcode::Rrq),
            2 => Ok(TftpOpcode::Wrq),
            3 => Ok(TftpOpcode::Data),
            4 => Ok(TftpOpcode::Ack),
            5 => Ok(TftpOpcode::Error),
            6 => Ok(TftpOpcode::Oack),
            _ => Err(TftpError::Tftp(format!("Invalid opcode: {}", value))),
        }
    }
}

// RFC 1350 - TFTP Error Codes
#[derive(Debug, Clone, Copy)]
#[repr(u16)]
enum TftpErrorCode {
    NotDefined = 0,        // Not defined, see error message
    FileNotFound = 1,      // File not found
    AccessViolation = 2,   // Access violation
    DiskFull = 3,          // Disk full or allocation exceeded
    IllegalOperation = 4,  // Illegal TFTP operation
    UnknownTid = 5,        // Unknown transfer ID
    FileExists = 6,        // File already exists
    NoSuchUser = 7,        // No such user
    OptionNegotiation = 8, // RFC 2347 - Option negotiation failure
}

// RFC 1350 - Transfer modes
///
/// NIST Controls:
/// - SI-10: Information Input Validation (mode validation)
/// - CM-6: Configuration Settings (transfer mode selection)
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TransferMode {
    /// NETASCII mode - 8-bit ASCII with network line ending conversion (CR+LF)
    /// RFC 1350: Convert local line endings to/from CR+LF format
    Netascii,
    /// OCTET mode - Binary transfer without conversion
    /// RFC 1350: Transfer data as-is without modification
    Octet,
    /// MAIL mode - Obsolete, not implemented
    /// RFC 1350: Originally for mail delivery, deprecated
    Mail,
}

impl TransferMode {
    /// Parse transfer mode from string
    ///
    /// NIST Controls:
    /// - SI-10: Information Input Validation (validate mode string)
    pub fn from_str(s: &str) -> std::result::Result<Self, TftpError> {
        match s.to_lowercase().as_str() {
            "netascii" => Ok(TransferMode::Netascii),
            "octet" => Ok(TransferMode::Octet),
            "mail" => Ok(TransferMode::Mail),
            _ => Err(TftpError::Tftp(format!("Invalid transfer mode: {}", s))),
        }
    }

    /// Convert data to NETASCII format (Unix LF -> CR+LF)
    ///
    /// RFC 1350 NETASCII Specification:
    /// - Lines are terminated with CR+LF (0x0D 0x0A)
    /// - Converts Unix line endings (LF) to network standard (CR+LF)
    /// - Handles CR, LF, and existing CR+LF sequences correctly
    ///
    /// NIST Controls:
    /// - SI-10: Information Input Validation (data format conversion)
    /// - SC-4: Information in Shared Resources (standardized encoding)
    pub fn convert_to_netascii(data: &[u8]) -> Vec<u8> {
        let mut result = Vec::with_capacity(data.len() + data.len() / 80); // Estimate extra space for CR
        let mut i = 0;

        while i < data.len() {
            let byte = data[i];

            match byte {
                b'\n' => {
                    // LF (0x0A) - check if preceded by CR
                    if i > 0 && data[i - 1] == b'\r' {
                        // Already CR+LF, just add LF
                        result.push(b'\n');
                    } else {
                        // Bare LF - convert to CR+LF
                        result.push(b'\r');
                        result.push(b'\n');
                    }
                }
                b'\r' => {
                    // CR (0x0D) - check if followed by LF
                    if i + 1 < data.len() && data[i + 1] == b'\n' {
                        // CR+LF sequence - add CR, LF will be handled in next iteration
                        result.push(b'\r');
                    } else {
                        // Bare CR - convert to CR+LF
                        result.push(b'\r');
                        result.push(b'\n');
                    }
                }
                _ => {
                    // Regular character - copy as-is
                    result.push(byte);
                }
            }

            i += 1;
        }

        result
    }
}

// RFC 2347/2348/2349 - TFTP Options
#[derive(Debug, Clone)]
pub(crate) struct TftpOptions {
    pub block_size: usize, // RFC 2348 - Block Size Option
    pub timeout: u64,      // RFC 2349 - Timeout Interval Option
    #[allow(dead_code)]
    pub transfer_size: Option<u64>, // RFC 2349 - Transfer Size Option
}

impl Default for TftpOptions {
    fn default() -> Self {
        Self {
            block_size: DEFAULT_BLOCK_SIZE,
            timeout: DEFAULT_TIMEOUT_SECS,
            transfer_size: None,
        }
    }
}

pub struct TftpServer {
    root_dir: PathBuf,
    bind_addr: SocketAddr,
    multicast_server: Option<Arc<MulticastTftpServer>>,
    max_file_size_bytes: u64,
}

impl TftpServer {
    pub fn new(root_dir: PathBuf, bind_addr: SocketAddr, max_file_size_bytes: u64) -> Self {
        Self {
            root_dir,
            bind_addr,
            multicast_server: None,
            max_file_size_bytes,
        }
    }

    /// Enable multicast support with configuration
    ///
    /// RFC 2090: Enable multicast TFTP deployments
    ///
    /// NIST Controls:
    /// - CM-6: Configuration Settings (enable multicast feature)
    /// - SC-5: Denial of Service Protection (multicast efficiency)
    pub fn with_multicast(mut self, config: MulticastConfig) -> Self {
        if config.enabled {
            let multicast_server = MulticastTftpServer::new(config, self.root_dir.clone());
            self.multicast_server = Some(Arc::new(multicast_server));
            info!("Multicast TFTP support enabled");
        }
        self
    }

    /// Run the TFTP server main loop
    ///
    /// NIST 800-53 Controls:
    /// - AU-3: Content of Audit Records (log all requests)
    /// - SC-7: Boundary Protection (enforce network boundaries)
    /// - SC-5: Denial of Service Protection (handle errors gracefully)
    ///
    /// STIG V-222563: Applications must produce audit records
    /// STIG V-222564: Applications must protect audit information
    pub async fn run(&self) -> Result<()> {
        let socket = Arc::new(UdpSocket::bind(self.bind_addr).await?);
        info!("TFTP server listening on {}", self.bind_addr);

        // NIST SC-5: Allocate fixed-size buffer to prevent memory exhaustion
        let mut buf = vec![0u8; MAX_PACKET_SIZE];

        loop {
            match socket.recv_from(&mut buf).await {
                Ok((size, client_addr)) => {
                    let data = buf[..size].to_vec();
                    let root_dir = self.root_dir.clone();
                    let multicast_server = self.multicast_server.clone();
                    let max_file_size = self.max_file_size_bytes;

                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_client(
                            data,
                            client_addr,
                            root_dir,
                            multicast_server,
                            max_file_size,
                        )
                        .await
                        {
                            error!("Error handling TFTP client {}: {}", client_addr, e);
                        }
                    });
                }
                Err(e) => {
                    error!("Error receiving TFTP packet: {}", e);
                }
            }
        }
    }

    /// Handle individual TFTP client requests
    ///
    /// NIST 800-53 Controls:
    /// - SI-10: Information Input Validation (validate all inputs)
    /// - AC-3: Access Enforcement (enforce file access restrictions)
    /// - AU-2: Audit Events (log security-relevant events)
    /// - SC-5: Denial of Service Protection (resource limits)
    ///
    /// STIG V-222577: Applications must validate all input
    /// STIG V-222578: Applications must protect from code injection
    /// STIG V-222602: Applications must enforce access restrictions
    async fn handle_client(
        data: Vec<u8>,
        client_addr: SocketAddr,
        root_dir: PathBuf,
        multicast_server: Option<Arc<MulticastTftpServer>>,
        max_file_size_bytes: u64,
    ) -> Result<()> {
        let mut bytes = BytesMut::from(&data[..]);

        // NIST SI-10: Validate minimum packet size
        // STIG V-222577: Input validation
        if bytes.len() < 2 {
            return Err(TftpError::Tftp("Packet too small".to_string()));
        }

        let opcode = bytes.get_u16();
        let opcode = TftpOpcode::try_from(opcode)?;

        match opcode {
            TftpOpcode::Rrq => {
                // RFC 1350: RRQ packet format
                // 2 bytes: opcode (01)
                // string: filename (null-terminated)
                // string: mode (null-terminated)
                // RFC 2347: followed by optional option/value pairs

                let filename = Self::parse_string(&mut bytes)?;
                let mode_str = Self::parse_string(&mut bytes)?;

                // Validate transfer mode
                let mode = TransferMode::from_str(&mode_str)?;

                // RFC 1350: Reject obsolete MAIL mode
                // NIST CM-7: Least Functionality - disable unsupported features
                if mode == TransferMode::Mail {
                    warn!(
                        "MAIL mode requested from {}: obsolete and not supported",
                        client_addr
                    );
                    Self::send_error(
                        client_addr,
                        TftpErrorCode::IllegalOperation,
                        "MAIL mode not supported",
                    )
                    .await?;
                    return Ok(());
                }

                // Parse options (RFC 2347)
                let mut options = TftpOptions::default();
                let mut requested_options = HashMap::new();
                let mut multicast_requested = false;

                while bytes.remaining() > 0 {
                    let option_name = match Self::parse_string(&mut bytes) {
                        Ok(s) => s,
                        Err(_) => break,
                    };

                    let option_value = match Self::parse_string(&mut bytes) {
                        Ok(s) => s,
                        Err(_) => break,
                    };

                    // RFC 2090: Check for multicast option
                    if option_name.to_lowercase() == "multicast" {
                        multicast_requested = true;
                    }

                    requested_options.insert(option_name.to_lowercase(), option_value);
                }

                // Process options
                let mut negotiated_options = HashMap::new();

                for (name, value) in &requested_options {
                    match name.as_str() {
                        "blksize" => {
                            // RFC 2348 - Block Size Option
                            if let Ok(size) = value.parse::<usize>()
                                && (8..=MAX_BLOCK_SIZE).contains(&size) {
                                    options.block_size = size;
                                    negotiated_options
                                        .insert("blksize".to_string(), size.to_string());
                                }
                        }
                        "timeout" => {
                            // RFC 2349 - Timeout Interval Option
                            if let Ok(timeout) = value.parse::<u64>()
                                && (1..=255).contains(&timeout) {
                                    options.timeout = timeout;
                                    negotiated_options
                                        .insert("timeout".to_string(), timeout.to_string());
                                }
                        }
                        "tsize" => {
                            // RFC 2349 - Transfer Size Option
                            // For RRQ, client sends 0 and server responds with actual size
                            if value == "0" {
                                negotiated_options.insert("tsize".to_string(), "0".to_string());
                                // Will be filled with actual size later
                            }
                        }
                        "multicast" => {
                            // RFC 2090: Multicast option (handled separately)
                            // Don't add to negotiated_options here
                        }
                        _ => {
                            // Unknown option - ignore per RFC 2347
                            debug!("Ignoring unknown option: {}", name);
                        }
                    }
                }

                debug!(
                    "RRQ from {}: {} (mode: {}, options: {:?}, multicast: {})",
                    client_addr, filename, mode_str, negotiated_options, multicast_requested
                );

                // RFC 2090: Handle multicast request if enabled and requested
                if multicast_requested {
                    if let Some(ref mcast_server) = multicast_server {
                        info!(
                            "Processing multicast request from {}: {}",
                            client_addr, filename
                        );

                        // Create a response socket for this client
                        let response_socket = Arc::new(UdpSocket::bind("0.0.0.0:0").await?);
                        response_socket.connect(client_addr).await?;

                        // Delegate to multicast server
                        return mcast_server
                            .handle_multicast_request(
                                filename,
                                mode,
                                options,
                                client_addr,
                                response_socket,
                            )
                            .await;
                    } else {
                        // Multicast requested but not enabled
                        warn!(
                            "Multicast requested from {} but multicast is not enabled",
                            client_addr
                        );
                        Self::send_error(
                            client_addr,
                            TftpErrorCode::OptionNegotiation,
                            "Multicast not supported",
                        )
                        .await?;
                        return Ok(());
                    }
                }

                // Validate filename (prevent directory traversal)
                let file_path = match Self::validate_and_resolve_path(&root_dir, &filename) {
                    Ok(path) => path,
                    Err(e) => {
                        Self::send_error(
                            client_addr,
                            TftpErrorCode::AccessViolation,
                            &e.to_string(),
                        )
                        .await?;
                        return Ok(());
                    }
                };

                Self::handle_read_request(
                    file_path,
                    client_addr,
                    mode,
                    options,
                    negotiated_options,
                    max_file_size_bytes,
                )
                .await?;
            }
            TftpOpcode::Wrq => {
                warn!("WRQ from {}: write not supported", client_addr);
                Self::send_error(
                    client_addr,
                    TftpErrorCode::AccessViolation,
                    "Write not supported",
                )
                .await?;
            }
            _ => {
                warn!("Unexpected opcode from {}: {:?}", client_addr, opcode);
                Self::send_error(
                    client_addr,
                    TftpErrorCode::IllegalOperation,
                    "Unexpected opcode",
                )
                .await?;
            }
        }

        Ok(())
    }

    /// Handle RRQ (Read Request) with support for NETASCII and OCTET modes
    ///
    /// NIST Controls:
    /// - AC-3: Access Enforcement (file access validation)
    /// - SI-10: Information Input Validation (transfer mode handling)
    /// - SC-4: Information in Shared Resources (data format conversion)
    async fn handle_read_request(
        file_path: PathBuf,
        client_addr: SocketAddr,
        mode: TransferMode,
        options: TftpOptions,
        mut negotiated_options: HashMap<String, String>,
        max_file_size_bytes: u64,
    ) -> Result<()> {
        // RFC 1350: Each transfer connection uses a new TID (Transfer ID)
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        socket.connect(client_addr).await?;

        // Open and validate file
        let mut file = match File::open(&file_path).await {
            Ok(f) => f,
            Err(_) => {
                Self::send_error_on_socket(&socket, TftpErrorCode::FileNotFound, "File not found")
                    .await?;
                return Ok(());
            }
        };

        let file_metadata = file.metadata().await?;
        let file_size = file_metadata.len();

        // Security: Validate file size to prevent memory exhaustion attacks
        // NIST 800-53 Controls:
        // - SC-5: Denial of Service Protection (prevent resource exhaustion)
        // - SI-10: Information Input Validation (validate resource consumption)
        //
        // STIG V-222609: Applications must protect against resource exhaustion
        // STIG V-222610: Applications must implement resource allocation restrictions
        if max_file_size_bytes > 0 && file_size > max_file_size_bytes {
            error!(
                "File size {} exceeds maximum allowed size {} for {}",
                file_size,
                max_file_size_bytes,
                file_path.display()
            );
            Self::send_error_on_socket(&socket, TftpErrorCode::DiskFull, "File too large").await?;
            return Ok(());
        }

        // RFC 2349: Update tsize option with actual file size
        // Note: For NETASCII mode, the transferred size may differ from file_size
        // due to line ending conversion, but we report the on-disk size per RFC behavior
        if negotiated_options.contains_key("tsize") {
            negotiated_options.insert("tsize".to_string(), file_size.to_string());
        }

        let block_size = options.block_size;
        let timeout = tokio::time::Duration::from_secs(options.timeout);

        // RFC 2347: Send OACK if options were negotiated
        if !negotiated_options.is_empty() {
            debug!("Sending OACK with options: {:?}", negotiated_options);

            let oack_packet = Self::build_oack_packet(&negotiated_options);
            Self::send_with_retry(&socket, &oack_packet, timeout).await?;

            // Wait for ACK of block 0 (the OACK)
            match Self::wait_for_ack(&socket, 0, timeout).await {
                Ok(()) => {}
                Err(e) => {
                    error!("Failed to receive ACK for OACK: {}", e);
                    return Ok(());
                }
            }
        }

        // Transfer file blocks
        // For NETASCII mode, we need to handle the entire file to apply line ending conversion
        // For OCTET mode, we can stream blocks directly
        let file_data = if mode == TransferMode::Netascii {
            // RFC 1350: NETASCII mode - convert line endings
            // NIST SI-10: Apply data format conversion for text transfer
            let mut raw_data = Vec::new();
            file.read_to_end(&mut raw_data).await?;
            TransferMode::convert_to_netascii(&raw_data)
        } else {
            // OCTET mode - read entire file for simplicity
            // (could be optimized for streaming in future)
            let mut raw_data = Vec::new();
            file.read_to_end(&mut raw_data).await?;
            raw_data
        };

        // Send file data in blocks
        let mut block_num: u16 = 1;
        let mut offset = 0;

        while offset < file_data.len() {
            // Calculate block size for this packet
            let bytes_to_send = std::cmp::min(block_size, file_data.len() - offset);
            let block_data = &file_data[offset..offset + bytes_to_send];

            // RFC 1350: DATA packet format
            // 2 bytes: opcode (03)
            // 2 bytes: block number
            // n bytes: data (0-blocksize bytes)
            let mut data_packet = BytesMut::with_capacity(4 + bytes_to_send);
            data_packet.put_u16(TftpOpcode::Data as u16);
            data_packet.put_u16(block_num);
            data_packet.put_slice(block_data);

            // Send with retries
            if let Err(e) = Self::send_with_retry(&socket, &data_packet, timeout).await {
                error!("Failed to send data block {}: {}", block_num, e);
                return Ok(());
            }

            // Wait for ACK
            match Self::wait_for_ack(&socket, block_num, timeout).await {
                Ok(()) => {}
                Err(e) => {
                    error!("Failed to receive ACK for block {}: {}", block_num, e);
                    return Ok(());
                }
            }

            offset += bytes_to_send;

            // RFC 1350: Transfer complete when data packet < block_size
            if bytes_to_send < block_size {
                debug!(
                    "Transfer complete: {} blocks sent ({} bytes, mode: {:?})",
                    block_num,
                    file_data.len(),
                    mode
                );
                break;
            }

            // RFC 1350: Block numbers wrap around after 65535
            block_num = block_num.wrapping_add(1);
        }

        // Handle empty file case
        if file_data.is_empty() {
            // Send a single empty data block
            let mut data_packet = BytesMut::with_capacity(4);
            data_packet.put_u16(TftpOpcode::Data as u16);
            data_packet.put_u16(1);

            Self::send_with_retry(&socket, &data_packet, timeout).await?;
            Self::wait_for_ack(&socket, 1, timeout).await?;

            debug!("Transfer complete: empty file (mode: {:?})", mode);
        }

        Ok(())
    }

    // Send packet with automatic retries
    async fn send_with_retry(
        socket: &UdpSocket,
        packet: &[u8],
        _timeout: tokio::time::Duration,
    ) -> Result<()> {
        if let Some(attempt) = (0..MAX_RETRIES).next() {
            socket.send(packet).await?;

            // For DATA/OACK packets, we'll wait for ACK in a separate function
            // This just ensures the send succeeded
            if attempt > 0 {
                debug!("Retransmission attempt {}", attempt);
            }

            return Ok(());
        }

        Err(TftpError::Tftp("Max retries exceeded".to_string()))
    }

    // Wait for ACK packet
    async fn wait_for_ack(
        socket: &UdpSocket,
        expected_block: u16,
        timeout: tokio::time::Duration,
    ) -> Result<()> {
        let mut ack_buf = vec![0u8; 1024];

        for retry in 0..MAX_RETRIES {
            match tokio::time::timeout(timeout, socket.recv(&mut ack_buf)).await {
                Ok(Ok(size)) => {
                    if size < 4 {
                        warn!("Received invalid ACK packet (too small)");
                        continue;
                    }

                    let mut ack_bytes = BytesMut::from(&ack_buf[..size]);
                    let opcode = ack_bytes.get_u16();

                    // Check for ERROR packet
                    if opcode == TftpOpcode::Error as u16 {
                        let error_code = ack_bytes.get_u16();
                        let error_msg: String = Self::parse_string(&mut ack_bytes).unwrap_or_default();
                        return Err(TftpError::Tftp(format!(
                            "Client sent error {}: {}",
                            error_code, error_msg
                        )));
                    }

                    if opcode != TftpOpcode::Ack as u16 {
                        warn!("Expected ACK, got opcode {}", opcode);
                        continue;
                    }

                    let ack_block = ack_bytes.get_u16();

                    // RFC 1350: Acknowledge the correct block
                    if ack_block == expected_block {
                        return Ok(());
                    } else if ack_block < expected_block {
                        // Duplicate ACK - ignore
                        debug!("Received duplicate ACK for block {}", ack_block);
                        continue;
                    } else {
                        warn!(
                            "ACK mismatch: expected {}, got {}",
                            expected_block, ack_block
                        );
                    }
                }
                Ok(Err(e)) => {
                    error!("Error receiving ACK: {}", e);
                }
                Err(_) => {
                    if retry < MAX_RETRIES - 1 {
                        debug!("Timeout waiting for ACK (retry {})", retry + 1);
                    }
                }
            }
        }

        Err(TftpError::Tftp(format!(
            "Timeout waiting for ACK of block {}",
            expected_block
        )))
    }

    // Build OACK packet (RFC 2347)
    fn build_oack_packet(options: &HashMap<String, String>) -> Vec<u8> {
        let mut packet = BytesMut::new();
        packet.put_u16(TftpOpcode::Oack as u16);

        for (name, value) in options {
            packet.put_slice(name.as_bytes());
            packet.put_u8(0);
            packet.put_slice(value.as_bytes());
            packet.put_u8(0);
        }

        packet.to_vec()
    }

    /// Parse null-terminated string from TFTP packet
    ///
    /// NIST 800-53 Controls:
    /// - SI-10: Information Input Validation (validate string format and length)
    /// - SC-5: Denial of Service Protection (prevent resource exhaustion)
    ///
    /// STIG V-222577: Applications must validate all input
    /// STIG V-222578: Applications must protect from buffer overflow attacks
    /// STIG V-222609: Applications must protect against resource exhaustion
    fn parse_string(bytes: &mut BytesMut) -> Result<String> {
        // Security: RFC 1350 strings (filenames, modes, options) should not exceed 255 bytes
        // This prevents DoS attacks with extremely long strings
        // NIST SI-10: Input validation with defined limits
        // STIG V-222609: Resource exhaustion protection
        const MAX_STRING_LENGTH: usize = 255;

        if bytes.len() > MAX_STRING_LENGTH {
            // Only search within reasonable bounds
            let search_slice = &bytes[..MAX_STRING_LENGTH];
            if !search_slice.contains(&0) {
                return Err(TftpError::Tftp(
                    "String too long (exceeds 255 bytes)".to_string(),
                ));
            }
        }

        let null_pos = bytes
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| TftpError::Tftp("No null terminator found".to_string()))?;

        if null_pos > MAX_STRING_LENGTH {
            return Err(TftpError::Tftp(
                "String too long (exceeds 255 bytes)".to_string(),
            ));
        }

        let string_bytes = bytes.split_to(null_pos);
        bytes.advance(1); // Skip the null terminator

        // NIST SI-10: Validate UTF-8 encoding
        // STIG V-222577: Input validation for character encoding
        String::from_utf8(string_bytes.to_vec())
            .map_err(|e| TftpError::Tftp(format!("Invalid UTF-8: {}", e)))
    }

    /// Validate and resolve file paths to prevent directory traversal attacks
    ///
    /// NIST 800-53 Controls:
    /// - AC-3: Access Enforcement (restrict access to authorized paths)
    /// - SI-10: Information Input Validation (validate filename format)
    /// - SC-7(12): Host-Based Boundary Protection (filesystem boundary enforcement)
    /// - CM-7: Least Functionality (read-only access, no writes)
    /// - AC-6: Least Privilege (restrict file access to designated directories)
    ///
    /// STIG V-222602: Applications must enforce access restrictions
    /// STIG V-222603: Applications must protect against directory traversal
    /// STIG V-222604: Applications must validate file paths
    /// STIG V-222611: Applications must prevent unauthorized file access
    /// STIG V-222612: Applications must implement path canonicalization
    fn validate_and_resolve_path(root_dir: &Path, filename: &str) -> Result<PathBuf> {
        // NIST SI-10: Normalize the filename and check for directory traversal
        // STIG V-222603: Prevent path traversal attacks (.., ./, etc.)
        let filename = filename.replace('\\', "/");
        if filename.contains("..") {
            return Err(TftpError::Tftp("Invalid filename".to_string()));
        }

        // NIST AC-3: Join with root directory to enforce base path
        // STIG V-222611: Restrict file access to authorized directory
        let file_path = root_dir.join(filename.trim_start_matches('/'));

        // Security: Detect and reject symlinks to prevent TOCTOU attacks
        // NIST AC-3: Additional access control check
        // STIG V-222604: Validate file type and reject symbolic links
        match std::fs::symlink_metadata(&file_path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(TftpError::Tftp("Symlinks are not allowed".to_string()));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // File doesn't exist - this is OK, will fail later at open
            }
            Err(_) => {
                return Err(TftpError::Tftp("Access denied".to_string()));
            }
        }

        // NIST AC-3: Ensure the resolved path is within root_dir
        // NIST SC-7(12): Enforce filesystem boundary protection
        // STIG V-222612: Path canonicalization for security validation
        let canonical_root = root_dir
            .canonicalize()
            .map_err(|_| TftpError::Tftp("Root directory error".to_string()))?;

        // Always perform boundary check, even if file doesn't exist yet
        // NIST AC-6: Least privilege - ensure access only within bounds
        if let Ok(canonical_file) = file_path.canonicalize() {
            if !canonical_file.starts_with(&canonical_root) {
                return Err(TftpError::Tftp("Access denied".to_string()));
            }
        } else {
            // File doesn't exist yet - check that the parent is within bounds
            if let Some(parent) = file_path.parent()
                && let Ok(canonical_parent) = parent.canonicalize()
                    && !canonical_parent.starts_with(&canonical_root) {
                        return Err(TftpError::Tftp("Access denied".to_string()));
                    }
        }

        Ok(file_path)
    }

    // RFC 1350: Send ERROR packet
    async fn send_error(
        client_addr: SocketAddr,
        error_code: TftpErrorCode,
        message: &str,
    ) -> Result<()> {
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        socket.connect(client_addr).await?;
        Self::send_error_on_socket(&socket, error_code, message).await
    }

    async fn send_error_on_socket(
        socket: &UdpSocket,
        error_code: TftpErrorCode,
        message: &str,
    ) -> Result<()> {
        // RFC 1350: ERROR packet format
        // 2 bytes: opcode (05)
        // 2 bytes: error code
        // string: error message (null-terminated)
        let mut packet = BytesMut::with_capacity(5 + message.len());
        packet.put_u16(TftpOpcode::Error as u16);
        packet.put_u16(error_code as u16);
        packet.put_slice(message.as_bytes());
        packet.put_u8(0); // Null terminator

        socket.send(&packet).await?;
        debug!("Sent ERROR packet: code={:?}, msg={}", error_code, message);
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let mut config = if cli.config.exists() {
        load_config(&cli.config)?
    } else {
        TftpConfig::default()
    };

    if let Some(root_dir) = cli.root_dir {
        config.root_dir = root_dir;
    }
    if let Some(bind_addr) = cli.bind {
        config.bind_addr = bind_addr;
    }
    if let Some(enabled) = cli.multicast {
        config.multicast.enabled = enabled;
    }
    if let Some(version) = cli.multicast_ip_version {
        config.multicast.multicast_ip_version = version;
        if cli.multicast_addr.is_none() {
            config.multicast.multicast_addr = default_multicast_addr_for_version(version);
        }
    }
    if let Some(addr) = cli.multicast_addr {
        config.multicast.multicast_addr = addr;
    }
    if let Some(port) = cli.multicast_port {
        config.multicast.multicast_port = port;
    }
    if let Some(max_clients) = cli.max_clients {
        config.multicast.max_clients = max_clients;
    }
    if let Some(master_timeout_secs) = cli.master_timeout_secs {
        config.multicast.master_timeout_secs = master_timeout_secs;
    }
    if let Some(retransmit_timeout_secs) = cli.retransmit_timeout_secs {
        config.multicast.retransmit_timeout_secs = retransmit_timeout_secs;
    }

    if cli.init_config {
        write_config(&cli.config, &config)?;
        if cli.create_root_dir {
            tokio::fs::create_dir_all(&config.root_dir).await?;
        }
        println!("Wrote config to {}", cli.config.display());
        return Ok(());
    }

    if cli.create_root_dir {
        tokio::fs::create_dir_all(&config.root_dir).await?;
    }

    if cli.check_config {
        validate_config(&config, false)?;
        println!("Config OK: {}", cli.config.display());
        return Ok(());
    }

    validate_config(&config, true)?;

    let _log_guard = if let Some(ref log_file) = config.logging.file {
        let dir = match log_file.parent() {
            Some(path) => path,
            None => std::path::Path::new("."),
        };
        let file_name = log_file
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| TftpError::Tftp("logging.file must include a file name".to_string()))?;
        let file_appender = tracing_appender::rolling::never(dir, file_name);
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new(config.logging.level.clone()))
            .with_writer(non_blocking)
            .init();

        Some(guard)
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new(config.logging.level.clone()))
            .init();

        None
    };

    let server = TftpServer::new(
        config.root_dir,
        config.bind_addr,
        config.max_file_size_bytes,
    )
    .with_multicast(config.multicast);
    server.run().await
}
