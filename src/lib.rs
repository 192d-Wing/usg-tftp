use bytes::{Buf, BufMut, BytesMut};
use snow_owl_core::*;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::net::UdpSocket;
use tracing::{debug, error, info, warn};

// RFC 1350 - The TFTP Protocol (Revision 2)
const TFTP_PORT: u16 = 69;
const DEFAULT_BLOCK_SIZE: usize = 512; // RFC 1350 standard block size
const MAX_BLOCK_SIZE: usize = 65464; // RFC 2348 maximum block size
const DEFAULT_TIMEOUT_SECS: u64 = 5;
const MAX_RETRIES: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TftpOpcode {
    Rrq = 1,   // Read request (RFC 1350)
    Wrq = 2,   // Write request (RFC 1350)
    Data = 3,  // Data packet (RFC 1350)
    Ack = 4,   // Acknowledgment (RFC 1350)
    Error = 5, // Error packet (RFC 1350)
    Oack = 6,  // Option acknowledgment (RFC 2347)
}

impl TryFrom<u16> for TftpOpcode {
    type Error = SnowOwlError;

    fn try_from(value: u16) -> std::result::Result<Self, Self::Error> {
        match value {
            1 => Ok(TftpOpcode::Rrq),
            2 => Ok(TftpOpcode::Wrq),
            3 => Ok(TftpOpcode::Data),
            4 => Ok(TftpOpcode::Ack),
            5 => Ok(TftpOpcode::Error),
            6 => Ok(TftpOpcode::Oack),
            _ => Err(SnowOwlError::Tftp(format!("Invalid opcode: {}", value))),
        }
    }
}

// RFC 1350 - TFTP Error Codes
#[derive(Debug, Clone, Copy)]
#[repr(u16)]
enum TftpErrorCode {
    NotDefined = 0,       // Not defined, see error message
    FileNotFound = 1,     // File not found
    AccessViolation = 2,  // Access violation
    DiskFull = 3,         // Disk full or allocation exceeded
    IllegalOperation = 4, // Illegal TFTP operation
    UnknownTid = 5,       // Unknown transfer ID
    FileExists = 6,       // File already exists
    NoSuchUser = 7,       // No such user
    OptionNegotiation = 8, // RFC 2347 - Option negotiation failure
}

// RFC 1350 - Transfer modes
#[derive(Debug, Clone, PartialEq, Eq)]
enum TransferMode {
    Netascii, // ASCII mode (line ending conversion)
    Octet,    // Binary mode (no conversion)
    Mail,     // Mail mode (obsolete)
}

impl TransferMode {
    fn from_str(s: &str) -> std::result::Result<Self, SnowOwlError> {
        match s.to_lowercase().as_str() {
            "netascii" => Ok(TransferMode::Netascii),
            "octet" => Ok(TransferMode::Octet),
            "mail" => Ok(TransferMode::Mail),
            _ => Err(SnowOwlError::Tftp(format!("Invalid transfer mode: {}", s))),
        }
    }
}

// RFC 2347/2348/2349 - TFTP Options
#[derive(Debug, Clone)]
struct TftpOptions {
    block_size: usize,      // RFC 2348 - Block Size Option
    timeout: u64,           // RFC 2349 - Timeout Interval Option
    transfer_size: Option<u64>, // RFC 2349 - Transfer Size Option
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
}

impl TftpServer {
    pub fn new(root_dir: PathBuf, bind_addr: SocketAddr) -> Self {
        Self {
            root_dir,
            bind_addr,
        }
    }

    pub async fn run(&self) -> Result<()> {
        let socket = UdpSocket::bind(self.bind_addr).await?;
        info!("TFTP server listening on {}", self.bind_addr);

        let mut buf = vec![0u8; MAX_PACKET_SIZE];

        loop {
            match socket.recv_from(&mut buf).await {
                Ok((size, client_addr)) => {
                    let data = buf[..size].to_vec();
                    let root_dir = self.root_dir.clone();

                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_client(data, client_addr, root_dir).await {
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

    async fn handle_client(data: Vec<u8>, client_addr: SocketAddr, root_dir: PathBuf) -> Result<()> {
        let mut bytes = BytesMut::from(&data[..]);

        if bytes.len() < 2 {
            return Err(SnowOwlError::Tftp("Packet too small".to_string()));
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

                // Parse options (RFC 2347)
                let mut options = TftpOptions::default();
                let mut requested_options = HashMap::new();

                while bytes.remaining() > 0 {
                    let option_name = match Self::parse_string(&mut bytes) {
                        Ok(s) => s,
                        Err(_) => break,
                    };

                    let option_value = match Self::parse_string(&mut bytes) {
                        Ok(s) => s,
                        Err(_) => break,
                    };

                    requested_options.insert(option_name.to_lowercase(), option_value);
                }

                // Process options
                let mut negotiated_options = HashMap::new();

                for (name, value) in &requested_options {
                    match name.as_str() {
                        "blksize" => {
                            // RFC 2348 - Block Size Option
                            if let Ok(size) = value.parse::<usize>() {
                                if size >= 8 && size <= MAX_BLOCK_SIZE {
                                    options.block_size = size;
                                    negotiated_options.insert("blksize".to_string(), size.to_string());
                                }
                            }
                        }
                        "timeout" => {
                            // RFC 2349 - Timeout Interval Option
                            if let Ok(timeout) = value.parse::<u64>() {
                                if timeout >= 1 && timeout <= 255 {
                                    options.timeout = timeout;
                                    negotiated_options.insert("timeout".to_string(), timeout.to_string());
                                }
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
                        _ => {
                            // Unknown option - ignore per RFC 2347
                            debug!("Ignoring unknown option: {}", name);
                        }
                    }
                }

                debug!(
                    "RRQ from {}: {} (mode: {}, options: {:?})",
                    client_addr, filename, mode_str, negotiated_options
                );

                // Validate filename (prevent directory traversal)
                let file_path = match Self::validate_and_resolve_path(&root_dir, &filename) {
                    Ok(path) => path,
                    Err(e) => {
                        Self::send_error(client_addr, TftpErrorCode::AccessViolation, &e.to_string()).await?;
                        return Ok(());
                    }
                };

                Self::handle_read_request(file_path, client_addr, mode, options, negotiated_options).await?;
            }
            TftpOpcode::Wrq => {
                warn!("WRQ from {}: write not supported", client_addr);
                Self::send_error(client_addr, TftpErrorCode::AccessViolation, "Write not supported").await?;
            }
            _ => {
                warn!("Unexpected opcode from {}: {:?}", client_addr, opcode);
                Self::send_error(client_addr, TftpErrorCode::IllegalOperation, "Unexpected opcode").await?;
            }
        }

        Ok(())
    }

    async fn handle_read_request(
        file_path: PathBuf,
        client_addr: SocketAddr,
        _mode: TransferMode,
        options: TftpOptions,
        mut negotiated_options: HashMap<String, String>,
    ) -> Result<()> {
        // RFC 1350: Each transfer connection uses a new TID (Transfer ID)
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        socket.connect(client_addr).await?;

        // Open and validate file
        let mut file = match File::open(&file_path).await {
            Ok(f) => f,
            Err(_) => {
                Self::send_error_on_socket(&socket, TftpErrorCode::FileNotFound, "File not found").await?;
                return Ok(());
            }
        };

        let file_metadata = file.metadata().await?;
        let file_size = file_metadata.len();

        // Update tsize option with actual file size
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
        let mut buffer = vec![0u8; block_size];
        let mut block_num: u16 = 1;

        loop {
            // Read a block from the file
            let bytes_read = file.read(&mut buffer).await?;

            // RFC 1350: DATA packet format
            // 2 bytes: opcode (03)
            // 2 bytes: block number
            // n bytes: data (0-blocksize bytes)
            let mut data_packet = BytesMut::with_capacity(4 + bytes_read);
            data_packet.put_u16(TftpOpcode::Data as u16);
            data_packet.put_u16(block_num);
            data_packet.put_slice(&buffer[..bytes_read]);

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

            // RFC 1350: Transfer complete when data packet < block_size
            if bytes_read < block_size {
                debug!("Transfer complete: {} blocks sent ({} bytes)", block_num, file_size);
                break;
            }

            // RFC 1350: Block numbers wrap around after 65535
            block_num = block_num.wrapping_add(1);
        }

        Ok(())
    }

    // Send packet with automatic retries
    async fn send_with_retry(socket: &UdpSocket, packet: &[u8], _timeout: tokio::time::Duration) -> Result<()> {
        for attempt in 0..MAX_RETRIES {
            socket.send(packet).await?;

            // For DATA/OACK packets, we'll wait for ACK in a separate function
            // This just ensures the send succeeded
            if attempt > 0 {
                debug!("Retransmission attempt {}", attempt);
            }

            return Ok(());
        }

        Err(SnowOwlError::Tftp("Max retries exceeded".to_string()))
    }

    // Wait for ACK packet
    async fn wait_for_ack(socket: &UdpSocket, expected_block: u16, timeout: tokio::time::Duration) -> Result<()> {
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
                        let error_msg = Self::parse_string(&mut ack_bytes).unwrap_or_default();
                        return Err(SnowOwlError::Tftp(format!(
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
                        warn!("ACK mismatch: expected {}, got {}", expected_block, ack_block);
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

        Err(SnowOwlError::Tftp(format!(
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

    fn parse_string(bytes: &mut BytesMut) -> Result<String> {
        let null_pos = bytes
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| SnowOwlError::Tftp("No null terminator found".to_string()))?;

        let string_bytes = bytes.split_to(null_pos);
        bytes.advance(1); // Skip the null terminator

        String::from_utf8(string_bytes.to_vec())
            .map_err(|e| SnowOwlError::Tftp(format!("Invalid UTF-8: {}", e)))
    }

    fn validate_and_resolve_path(root_dir: &Path, filename: &str) -> Result<PathBuf> {
        // Normalize the filename and check for directory traversal
        let filename = filename.replace('\\', "/");
        if filename.contains("..") {
            return Err(SnowOwlError::Tftp("Invalid filename".to_string()));
        }

        let file_path = root_dir.join(filename.trim_start_matches('/'));

        // Ensure the resolved path is within root_dir
        let canonical_root = root_dir.canonicalize().unwrap_or_else(|_| root_dir.to_path_buf());
        if let Ok(canonical_file) = file_path.canonicalize() {
            if !canonical_file.starts_with(&canonical_root) {
                return Err(SnowOwlError::Tftp("Access denied".to_string()));
            }
        }

        Ok(file_path)
    }

    // RFC 1350: Send ERROR packet
    async fn send_error(client_addr: SocketAddr, error_code: TftpErrorCode, message: &str) -> Result<()> {
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        socket.connect(client_addr).await?;
        Self::send_error_on_socket(&socket, error_code, message).await
    }

    async fn send_error_on_socket(socket: &UdpSocket, error_code: TftpErrorCode, message: &str) -> Result<()> {
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
