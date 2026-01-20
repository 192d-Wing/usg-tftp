// Allow unused code for items that are part of the public API or reserved for future use
#![allow(dead_code)]

// Public modules - shared between server and client
pub mod audit;
pub mod buffer_pool;
pub mod config;
pub mod error;
pub mod multicast;
pub mod worker_pool;

// Server module stub (to be properly implemented)
pub mod server {
    use super::*;
    use std::path::{Path, PathBuf};

    pub struct TftpServer;

    impl TftpServer {
        // Placeholder - will be implemented properly in server binary
        pub fn validate_and_resolve_path(_root: &Path, _filename: &str) -> Result<PathBuf> {
            Err(TftpError::Tftp("Not implemented in library".into()))
        }

        // Placeholder - will be implemented properly in server binary
        pub async fn handle_read_request(
            _file_path: PathBuf,
            _client_addr: std::net::SocketAddr,
            _mode: TransferMode,
            _options: TftpOptions,
            _negotiated_options: std::collections::HashMap<String, String>,
            _max_file_size: u64,
            _audit_enabled: bool,
            _file_io_config: &config::FileIoConfig,
        ) -> Result<()> {
            Err(TftpError::Tftp("Not implemented in library".into()))
        }

        // Placeholder - will be implemented properly in server binary
        pub async fn handle_write_request(
            _file_path: PathBuf,
            _client_addr: std::net::SocketAddr,
            _mode: TransferMode,
            _options: TftpOptions,
            _negotiated_options: std::collections::HashMap<String, String>,
            _max_file_size: u64,
            _file_created: bool,
            _audit_enabled: bool,
        ) -> Result<()> {
            Err(TftpError::Tftp("Not implemented in library".into()))
        }
    }
}

pub use server::TftpServer;

// Re-export commonly used types
pub use error::{Result, TftpError};
pub use config::TftpConfig;

// RFC 1350 - The TFTP Protocol (Revision 2)
pub const DEFAULT_BLOCK_SIZE: usize = 512; // RFC 1350 standard for compatibility
pub const MAX_BLOCK_SIZE: usize = 65464; // RFC 2348 maximum block size
pub const MAX_PACKET_SIZE: usize = 65468; // Max block size + 4 byte header
pub const DEFAULT_TIMEOUT_SECS: u64 = 5;
pub const MAX_RETRIES: u32 = 5;

// TFTP Opcodes (RFC 1350)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Opcode {
    Rrq = 1,   // Read Request
    Wrq = 2,   // Write Request
    Data = 3,  // Data
    Ack = 4,   // Acknowledgment
    Error = 5, // Error
    Oack = 6,  // Option Acknowledgment (RFC 2347)
}

// Type alias for backward compatibility
pub type TftpOpcode = Opcode;

impl Opcode {
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            1 => Some(Opcode::Rrq),
            2 => Some(Opcode::Wrq),
            3 => Some(Opcode::Data),
            4 => Some(Opcode::Ack),
            5 => Some(Opcode::Error),
            6 => Some(Opcode::Oack),
            _ => None,
        }
    }
}

// TFTP Error Codes (RFC 1350)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ErrorCode {
    NotDefined = 0,
    FileNotFound = 1,
    AccessViolation = 2,
    DiskFull = 3,
    IllegalOperation = 4,
    UnknownTransferId = 5,
    FileAlreadyExists = 6,
    NoSuchUser = 7,
    OptionNegotiationFailed = 8, // RFC 2347
}

impl ErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorCode::NotDefined => "Not defined",
            ErrorCode::FileNotFound => "File not found",
            ErrorCode::AccessViolation => "Access violation",
            ErrorCode::DiskFull => "Disk full or allocation exceeded",
            ErrorCode::IllegalOperation => "Illegal TFTP operation",
            ErrorCode::UnknownTransferId => "Unknown transfer ID",
            ErrorCode::FileAlreadyExists => "File already exists",
            ErrorCode::NoSuchUser => "No such user",
            ErrorCode::OptionNegotiationFailed => "Option negotiation failed",
        }
    }
}

// Transfer Mode (RFC 1350)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferMode {
    /// NETASCII mode - 8-bit ASCII with network line ending conversion (CR+LF)
    Netascii,
    /// OCTET mode - Binary transfer without conversion
    Octet,
    /// MAIL mode - Obsolete, not implemented
    Mail,
}

impl TransferMode {
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "netascii" => Ok(TransferMode::Netascii),
            "octet" => Ok(TransferMode::Octet),
            "mail" => Ok(TransferMode::Mail),
            _ => Err(TftpError::Tftp(format!("Unknown transfer mode: {}", s))),
        }
    }

    /// Convert binary data to NETASCII format (RFC 1350)
    pub fn convert_to_netascii(data: &[u8]) -> Vec<u8> {
        let mut result = Vec::with_capacity(data.len());
        for &byte in data {
            match byte {
                b'\n' => {
                    result.push(b'\r');
                    result.push(b'\n');
                }
                b'\r' => {
                    result.push(b'\r');
                    result.push(b'\0');
                }
                _ => result.push(byte),
            }
        }
        result
    }
}

// TFTP Options (RFC 2347/2348/2349/7440)
#[derive(Debug, Clone)]
pub struct TftpOptions {
    pub block_size: usize,              // RFC 2348 - Block Size Option
    pub timeout: u64,                   // RFC 2349 - Timeout Interval Option
    pub transfer_size: Option<u64>,     // RFC 2349 - Transfer Size Option
    pub windowsize: usize,              // RFC 7440 - Windowsize Option (1-65535 blocks)
}

impl Default for TftpOptions {
    fn default() -> Self {
        Self {
            block_size: DEFAULT_BLOCK_SIZE,
            timeout: DEFAULT_TIMEOUT_SECS,
            transfer_size: None,
            windowsize: 1,
        }
    }
}
