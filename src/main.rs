// Allow unused code for items that are part of the public API or reserved for future use
#![allow(dead_code)]

mod audit;
mod buffer_pool;
mod config;
mod error;
mod multicast;
mod worker_pool;

use audit::AuditLogger;
use buffer_pool::BufferPool;
use bytes::{Buf, BufMut, BytesMut};
use clap::Parser;
use config::{
    LogFormat, MulticastConfig, MulticastIpVersion, SocketConfig, TftpConfig, WriteConfig,
    default_multicast_addr_for_version, load_config, validate_config, write_config,
};
use error::{Result, TftpError};
use multicast::MulticastTftpServer;
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// Phase 2: Batch operations and zero-copy transfers
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use nix::sys::socket::{MsgFlags, MultiHeaders, SockaddrStorage, recvmmsg, sendmmsg};
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use std::os::unix::io::AsRawFd;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
// RFC 1350: Default block size is 512 bytes for compatibility
// RFC 2348: Clients can negotiate larger blocks (up to 65464 bytes) via blksize option
// Performance note: Configure clients to request blksize=8192 or higher for better throughput
pub(crate) const DEFAULT_BLOCK_SIZE: usize = 512; // RFC 1350 standard for compatibility
const MAX_BLOCK_SIZE: usize = 65464; // RFC 2348 maximum block size
const MAX_PACKET_SIZE: usize = 65468; // Max block size + 4 byte header
const DEFAULT_TIMEOUT_SECS: u64 = 5;
const MAX_RETRIES: u32 = 5;

/// Apply POSIX file advisory hints for optimal kernel I/O behavior
///
/// Phase 1 Performance Enhancement: File I/O Optimization
/// - POSIX_FADV_SEQUENTIAL: Optimize for sequential reads
/// - POSIX_FADV_WILLNEED: Prefetch file data
/// - POSIX_FADV_DONTNEED: Free page cache after transfer (optional)
///
/// Expected Impact:
/// - 20-30% reduction in read latency
/// - Optimized kernel read-ahead behavior
#[cfg(target_os = "linux")]
fn apply_file_hints(file: &File, config: &config::FileIoConfig, file_size: u64) {
    use std::os::unix::io::AsRawFd;
    // Manually call posix_fadvise via libc since nix doesn't expose it directly
    const POSIX_FADV_SEQUENTIAL: libc::c_int = 2;
    const POSIX_FADV_WILLNEED: libc::c_int = 3;

    let fd = file.as_raw_fd();

    if config.use_sequential_hint {
        let result = unsafe { libc::posix_fadvise(fd, 0, file_size as i64, POSIX_FADV_SEQUENTIAL) };
        if result != 0 {
            debug!("Failed to set POSIX_FADV_SEQUENTIAL: errno {}", result);
        } else {
            debug!("Applied POSIX_FADV_SEQUENTIAL hint for optimal sequential reading");
        }
    }

    if config.use_willneed_hint {
        let result = unsafe { libc::posix_fadvise(fd, 0, file_size as i64, POSIX_FADV_WILLNEED) };
        if result != 0 {
            debug!("Failed to set POSIX_FADV_WILLNEED: errno {}", result);
        } else {
            debug!("Applied POSIX_FADV_WILLNEED hint to prefetch file data");
        }
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn apply_file_hints(_file: &File, _config: &config::FileIoConfig, _file_size: u64) {
    // posix_fadvise implementation varies on BSD, using Linux-only for now
}

#[cfg(not(unix))]
fn apply_file_hints(_file: &File, _config: &config::FileIoConfig, _file_size: u64) {
    // File hints not available on non-Unix platforms
}

/// Release file from page cache after transfer completes
#[cfg(target_os = "linux")]
fn release_file_cache(file: &File, file_size: u64) {
    use std::os::unix::io::AsRawFd;
    const POSIX_FADV_DONTNEED: libc::c_int = 4;

    let fd = file.as_raw_fd();
    let result = unsafe { libc::posix_fadvise(fd, 0, file_size as i64, POSIX_FADV_DONTNEED) };
    if result != 0 {
        debug!("Failed to set POSIX_FADV_DONTNEED: errno {}", result);
    } else {
        debug!("Released file from page cache (POSIX_FADV_DONTNEED)");
    }
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
fn release_file_cache(_file: &File, _file_size: u64) {
    // File cache release not available on non-Linux platforms (for now)
}

/// Batch receive multiple packets using recvmmsg() (Phase 2)
///
/// Reduces syscall overhead by receiving multiple packets in a single syscall.
/// Expected improvement: 60-80% reduction in syscall overhead for concurrent transfers.
///
/// Linux: Available since 2.6.33
/// FreeBSD: Available since 11.0
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn batch_recv_packets(
    socket: &UdpSocket,
    buffers: &mut [Vec<u8>],
    max_packets: usize,
    timeout_us: u64,
) -> Result<Vec<(usize, SocketAddr)>> {
    use nix::sys::time::TimeSpec;
    use std::io::IoSliceMut;
    use std::time::Duration;

    let socket_fd = socket.as_raw_fd();
    let batch_size = std::cmp::min(max_packets, buffers.len());

    debug!(
        "batch_recv_packets called: fd={}, batch_size={}, timeout={}μs",
        socket_fd, batch_size, timeout_us
    );

    // Prepare RecvMmsgData structures
    let mut iovecs: Vec<Vec<IoSliceMut>> = buffers[..batch_size]
        .iter_mut()
        .map(|buf| vec![IoSliceMut::new(buf)])
        .collect();

    let mut headers = MultiHeaders::<SockaddrStorage>::preallocate(batch_size, None);

    // Use timeout instead of MSG_DONTWAIT to allow packets to accumulate
    let timeout = if timeout_us > 0 {
        Some(TimeSpec::from_duration(Duration::from_micros(timeout_us)))
    } else {
        None
    };

    debug!("Calling recvmmsg() syscall with timeout...");
    // Perform batch receive
    match recvmmsg(
        socket_fd,
        &mut headers,
        iovecs.iter_mut(),
        MsgFlags::empty(), // Don't use MSG_DONTWAIT - let timeout handle waiting
        timeout,
    ) {
        Ok(msgs_received) => {
            let mut results = Vec::new();

            for msg in msgs_received {
                if let Some(addr_storage) = msg.address {
                    // Convert SockaddrStorage to SocketAddr
                    if let Some(sock_addr) = addr_storage.as_sockaddr_in() {
                        let addr =
                            SocketAddr::new(IpAddr::V4(sock_addr.ip().into()), sock_addr.port());
                        results.push((msg.bytes, addr));
                    } else if let Some(sock_addr) = addr_storage.as_sockaddr_in6() {
                        let addr =
                            SocketAddr::new(IpAddr::V6(sock_addr.ip().into()), sock_addr.port());
                        results.push((msg.bytes, addr));
                    }
                }
            }

            debug!("Received {} packets in batch via recvmmsg()", results.len());
            Ok(results)
        }
        #[allow(unreachable_patterns)]
        Err(nix::errno::Errno::EAGAIN) | Err(nix::errno::Errno::EWOULDBLOCK) => {
            // No packets available
            Ok(Vec::new())
        }
        Err(e) => Err(TftpError::Tftp(format!("recvmmsg error: {}", e))),
    }
}

/// Batch send multiple packets using sendmmsg() (Phase 2)
///
/// Reduces syscall overhead by sending multiple packets in a single syscall.
/// Most beneficial during concurrent transfers or multicast operations.
/// Expected improvement: 60-80% reduction in syscall overhead.
///
/// Linux: Available since 3.0
/// FreeBSD: Available since 11.0
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn batch_send_packets(socket: &UdpSocket, packets: &[(Vec<u8>, SocketAddr)]) -> Result<usize> {
    use nix::sys::socket::ControlMessage;
    use std::io::IoSlice;

    let socket_fd = socket.as_raw_fd();

    let mut iovecs: Vec<Vec<IoSlice>> = Vec::with_capacity(packets.len());
    let mut addrs: Vec<Option<SockaddrStorage>> = Vec::with_capacity(packets.len());

    for (data, addr) in packets.iter() {
        iovecs.push(vec![IoSlice::new(data)]);
        addrs.push(Some(SockaddrStorage::from(*addr)));
    }

    let mut headers = MultiHeaders::<SockaddrStorage>::preallocate(packets.len(), None);
    let cmsgs: [ControlMessage; 0] = [];

    // Perform batch send
    match sendmmsg(
        socket_fd,
        &mut headers,
        iovecs.iter(),
        &addrs,
        &cmsgs,
        MsgFlags::empty(),
    ) {
        Ok(results) => {
            let sent_count = results.count();
            debug!("Sent {} packets in batch via sendmmsg()", sent_count);
            Ok(sent_count)
        }
        Err(e) => Err(TftpError::Tftp(format!("sendmmsg error: {}", e))),
    }
}

/// Create an optimized UDP socket with platform-specific performance tuning
///
/// Phase 1 Performance Enhancements:
/// - SO_RCVBUF/SO_SNDBUF: Increase socket buffers to reduce packet drops
/// - SO_REUSEADDR: Enable faster server restarts
/// - SO_REUSEPORT: Enable multi-process scaling (Linux 3.9+, BSD)
///
/// NIST 800-53 SC-5: Denial of Service Protection (buffer sizing)
fn create_optimized_socket(bind_addr: SocketAddr, config: &SocketConfig) -> Result<UdpSocket> {
    let domain = if bind_addr.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };

    let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))
        .map_err(|e| TftpError::Tftp(format!("Failed to create socket: {}", e)))?;

    // Phase 1.1: SO_REUSEADDR - Enable address reuse for faster restarts
    if config.reuse_address {
        socket
            .set_reuse_address(true)
            .map_err(|e| TftpError::Tftp(format!("Failed to set SO_REUSEADDR: {}", e)))?;
        debug!("Enabled SO_REUSEADDR for faster server restarts");
    }

    // Phase 1.2: SO_REUSEPORT - Enable multi-process scaling
    // Available on Linux 3.9+, FreeBSD, OpenBSD, NetBSD
    #[cfg(all(unix, not(target_os = "solaris"), not(target_os = "illumos")))]
    if config.reuse_port {
        socket
            .set_reuse_port(true)
            .map_err(|e| {
                warn!("Failed to set SO_REUSEPORT (may not be supported): {}", e);
                // Don't fail - this is an optimization, not required
            })
            .ok();
        debug!("Enabled SO_REUSEPORT for multi-process scaling");
    }

    // Phase 1.3: SO_RCVBUF - Increase receive buffer to reduce packet drops
    let recv_buffer_bytes = config.recv_buffer_kb * 1024;
    socket
        .set_recv_buffer_size(recv_buffer_bytes)
        .map_err(|e| {
            warn!(
                "Failed to set SO_RCVBUF to {} KB: {}",
                config.recv_buffer_kb, e
            );
        })
        .ok();

    // Verify actual buffer size (kernel may adjust)
    if let Ok(actual_size) = socket.recv_buffer_size() {
        info!(
            "Socket receive buffer: requested {} KB, actual {} KB",
            config.recv_buffer_kb,
            actual_size / 1024
        );
    }

    // Phase 1.4: SO_SNDBUF - Increase send buffer for better burst handling
    let send_buffer_bytes = config.send_buffer_kb * 1024;
    socket
        .set_send_buffer_size(send_buffer_bytes)
        .map_err(|e| {
            warn!(
                "Failed to set SO_SNDBUF to {} KB: {}",
                config.send_buffer_kb, e
            );
        })
        .ok();

    // Verify actual buffer size
    if let Ok(actual_size) = socket.send_buffer_size() {
        info!(
            "Socket send buffer: requested {} KB, actual {} KB",
            config.send_buffer_kb,
            actual_size / 1024
        );
    }

    // Bind the socket
    socket
        .bind(&bind_addr.into())
        .map_err(|e| TftpError::Tftp(format!("Failed to bind to {}: {}", bind_addr, e)))?;

    // Set non-blocking mode for tokio
    socket
        .set_nonblocking(true)
        .map_err(|e| TftpError::Tftp(format!("Failed to set non-blocking: {}", e)))?;

    // Convert socket2::Socket to std::net::UdpSocket, then to tokio::net::UdpSocket
    let std_socket: std::net::UdpSocket = socket.into();
    let tokio_socket = UdpSocket::from_std(std_socket)
        .map_err(|e| TftpError::Tftp(format!("Failed to convert to tokio socket: {}", e)))?;

    Ok(tokio_socket)
}

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
    ///
    /// Performance optimization: Process in larger chunks for better CPU cache utilization
    pub fn convert_to_netascii(data: &[u8]) -> Vec<u8> {
        if data.is_empty() {
            return Vec::new();
        }

        // Pre-allocate with better size estimation: assume 10% line endings
        let mut result = Vec::with_capacity(data.len() + data.len() / 10);

        // Process in chunks for better cache utilization
        const CHUNK_SIZE: usize = 4096;
        let mut i = 0;

        while i < data.len() {
            let chunk_end = std::cmp::min(i + CHUNK_SIZE, data.len());
            let chunk = &data[i..chunk_end];

            // Fast path: scan for line endings first
            let mut last_copy = 0;
            for (idx, &byte) in chunk.iter().enumerate() {
                match byte {
                    b'\n' => {
                        // Copy everything up to this point
                        result.extend_from_slice(&chunk[last_copy..idx]);

                        // Check if preceded by CR
                        let preceded_by_cr = if idx > 0 {
                            chunk[idx - 1] == b'\r'
                        } else if i > 0 && !result.is_empty() {
                            result[result.len() - 1] == b'\r'
                        } else {
                            false
                        };

                        if preceded_by_cr {
                            // Already CR+LF, just add LF
                            result.push(b'\n');
                        } else {
                            // Bare LF - convert to CR+LF
                            result.push(b'\r');
                            result.push(b'\n');
                        }

                        last_copy = idx + 1;
                    }
                    b'\r' => {
                        // Copy everything up to this point
                        result.extend_from_slice(&chunk[last_copy..idx]);

                        // Check if followed by LF
                        let followed_by_lf = if idx + 1 < chunk.len() {
                            chunk[idx + 1] == b'\n'
                        } else if chunk_end < data.len() {
                            data[chunk_end] == b'\n'
                        } else {
                            false
                        };

                        if followed_by_lf {
                            // CR+LF sequence - add CR, LF will be handled in next iteration
                            result.push(b'\r');
                        } else {
                            // Bare CR - convert to CR+LF
                            result.push(b'\r');
                            result.push(b'\n');
                        }

                        last_copy = idx + 1;
                    }
                    _ => {
                        // Continue scanning
                    }
                }
            }

            // Copy remaining chunk data
            result.extend_from_slice(&chunk[last_copy..]);
            i = chunk_end;
        }

        result
    }
}

// RFC 2347/2348/2349/7440 - TFTP Options
#[derive(Debug, Clone)]
pub(crate) struct TftpOptions {
    pub block_size: usize, // RFC 2348 - Block Size Option
    pub timeout: u64,      // RFC 2349 - Timeout Interval Option
    #[allow(dead_code)]
    pub transfer_size: Option<u64>, // RFC 2349 - Transfer Size Option
    pub windowsize: usize, // RFC 7440 - Windowsize Option (1-65535 blocks)
}

impl Default for TftpOptions {
    fn default() -> Self {
        Self {
            block_size: DEFAULT_BLOCK_SIZE,
            timeout: DEFAULT_TIMEOUT_SECS,
            transfer_size: None,
            windowsize: 1, // RFC 7440: windowsize=1 equals RFC 1350 behavior
        }
    }
}

pub struct TftpServer {
    root_dir: PathBuf,
    bind_addr: SocketAddr,
    multicast_server: Option<Arc<MulticastTftpServer>>,
    max_file_size_bytes: u64,
    write_config: WriteConfig,
    audit_enabled: bool,
    buffer_pool: BufferPool,
    config: Arc<TftpConfig>,
    active_clients: Arc<AtomicUsize>,
}

impl TftpServer {
    pub fn new(
        root_dir: PathBuf,
        bind_addr: SocketAddr,
        max_file_size_bytes: u64,
        write_config: WriteConfig,
        audit_enabled: bool,
        config: Arc<TftpConfig>,
    ) -> Self {
        Self {
            root_dir,
            bind_addr,
            multicast_server: None,
            max_file_size_bytes,
            write_config,
            audit_enabled,
            buffer_pool: BufferPool::new_default(),
            config,
            active_clients: Arc::new(AtomicUsize::new(0)),
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
            let multicast_server =
                MulticastTftpServer::new(config, self.root_dir.clone(), self.audit_enabled);
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
        // Phase 1: Create optimized socket with platform-specific performance tuning
        let socket = Arc::new(create_optimized_socket(
            self.bind_addr,
            &self.config.performance.platform.socket,
        )?);
        info!("TFTP server listening on {}", self.bind_addr);

        // Performance optimization: Use buffer pool to avoid allocations
        let buffer_pool = self.buffer_pool.clone();
        let active_clients = self.active_clients.clone();

        // Phase 2: Batch receiving configuration
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let batch_config = &self.config.performance.platform.batch;
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let batch_size = batch_config.max_batch_size;
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let batch_timeout_us = batch_config.batch_timeout_us;
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let adaptive_batching_enabled = batch_config.enable_adaptive_batching;
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let adaptive_threshold = batch_config.adaptive_batch_threshold;
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let base_batch_enabled = batch_config.enable_recvmmsg;

        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        let adaptive_batching_enabled = false;

        if adaptive_batching_enabled {
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            info!(
                "Adaptive batching enabled: batch receiving will be used when active clients >= {}",
                adaptive_threshold
            );
        } else {
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            if base_batch_enabled {
                info!("Using recvmmsg() batch receiving (Phase 2 optimization)");
            }
        }

        loop {
            // Phase 2: Adaptive batching - decide whether to use batch receiving
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            let use_batch_recv = if adaptive_batching_enabled {
                let current_clients = active_clients.load(Ordering::Relaxed);
                let should_batch = current_clients >= adaptive_threshold && base_batch_enabled;
                debug!(
                    "Adaptive batching: clients={}, threshold={}, base_enabled={}, will_use_batch={}",
                    current_clients, adaptive_threshold, base_batch_enabled, should_batch
                );
                should_batch
            } else {
                debug!("Using fixed batching mode: enabled={}", base_batch_enabled);
                base_batch_enabled
            };

            #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
            let use_batch_recv = false;

            // Phase 2: Try batch receive first on supported platforms
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            if use_batch_recv {
                debug!("Attempting batch receive with batch_size={}", batch_size);
                // Prepare batch buffers
                let mut buffers: Vec<Vec<u8>> = (0..batch_size)
                    .map(|_| vec![0u8; MAX_PACKET_SIZE])
                    .collect();

                match batch_recv_packets(&socket, &mut buffers, batch_size, batch_timeout_us) {
                    Ok(packets) if !packets.is_empty() => {
                        // Process each received packet
                        for (i, (size, client_addr)) in packets.iter().enumerate() {
                            let mut buf = buffer_pool.acquire().await;
                            buf.clear();
                            buf.extend_from_slice(&buffers[i][..*size]);

                            let root_dir = self.root_dir.clone();
                            let multicast_server = self.multicast_server.clone();
                            let max_file_size = self.max_file_size_bytes;
                            let write_config = self.write_config.clone();
                            let audit_enabled = self.audit_enabled;
                            let file_io_config = self.config.performance.platform.file_io.clone();
                            let default_windowsize = self.config.performance.default_windowsize;
                            let pool = buffer_pool.clone();
                            let addr = *client_addr;
                            let client_counter = active_clients.clone();

                            // Increment active clients counter
                            client_counter.fetch_add(1, Ordering::Relaxed);

                            tokio::spawn(async move {
                                if let Err(e) = Self::handle_client(
                                    buf.to_vec(),
                                    addr,
                                    root_dir,
                                    multicast_server,
                                    max_file_size,
                                    write_config,
                                    audit_enabled,
                                    file_io_config,
                                    default_windowsize,
                                )
                                .await
                                {
                                    error!("Error handling TFTP client {}: {}", addr, e);
                                }
                                pool.release(buf).await;

                                // Decrement active clients counter when done
                                client_counter.fetch_sub(1, Ordering::Relaxed);
                            });
                        }
                        continue;
                    }
                    Ok(_) => {
                        // Timeout expired with no packets - retry batch receive
                        debug!("Batch receive timeout, retrying...");
                        continue;
                    }
                    Err(e) => {
                        warn!("Batch receive error ({}), falling back to single recv", e);
                        // Fall through to single recv_from on actual errors
                    }
                }
            }

            // Fallback or default: single packet receive
            let mut buf = buffer_pool.acquire().await;
            buf.resize(MAX_PACKET_SIZE, 0);

            match socket.recv_from(&mut buf).await {
                Ok((size, client_addr)) => {
                    // Take ownership of the data without copying
                    let mut data = buf;
                    data.truncate(size);

                    let root_dir = self.root_dir.clone();
                    let multicast_server = self.multicast_server.clone();
                    let max_file_size = self.max_file_size_bytes;
                    let write_config = self.write_config.clone();
                    let audit_enabled = self.audit_enabled;
                    let file_io_config = self.config.performance.platform.file_io.clone();
                    let default_windowsize = self.config.performance.default_windowsize;
                    let pool = buffer_pool.clone();
                    let client_counter = active_clients.clone();

                    // Increment active clients counter
                    client_counter.fetch_add(1, Ordering::Relaxed);

                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_client(
                            data.to_vec(),
                            client_addr,
                            root_dir,
                            multicast_server,
                            max_file_size,
                            write_config,
                            audit_enabled,
                            file_io_config,
                            default_windowsize,
                        )
                        .await
                        {
                            error!("Error handling TFTP client {}: {}", client_addr, e);
                        }
                        // Buffer will be returned to pool when dropped
                        pool.release(data).await;

                        // Decrement active clients counter when done
                        client_counter.fetch_sub(1, Ordering::Relaxed);
                    });
                }
                Err(e) => {
                    error!("Error receiving TFTP packet: {}", e);
                    // Return buffer to pool on error
                    buffer_pool.release(buf).await;
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
    #[allow(clippy::too_many_arguments)]
    async fn handle_client(
        data: Vec<u8>,
        client_addr: SocketAddr,
        root_dir: PathBuf,
        multicast_server: Option<Arc<MulticastTftpServer>>,
        max_file_size_bytes: u64,
        write_config: WriteConfig,
        audit_enabled: bool,
        file_io_config: config::FileIoConfig,
        default_windowsize: usize,
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
                let mut options = TftpOptions {
                    windowsize: default_windowsize,
                    ..TftpOptions::default()
                };
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

                // RFC 2347: Option negotiation
                // Server MUST either accept option with valid value or omit from OACK
                for (name, value) in &requested_options {
                    match name.as_str() {
                        "blksize" => {
                            // RFC 2348 - Block Size Option (valid range: 8-65464 bytes)
                            match value.parse::<usize>() {
                                Ok(size) if (8..=MAX_BLOCK_SIZE).contains(&size) => {
                                    options.block_size = size;
                                    negotiated_options
                                        .insert("blksize".to_string(), size.to_string());
                                }
                                Ok(size) => {
                                    // Invalid size - log and omit from OACK per RFC 2347
                                    warn!(
                                        "Client {} requested invalid blksize={} (valid: 8-{}), using default {}",
                                        client_addr, size, MAX_BLOCK_SIZE, options.block_size
                                    );
                                }
                                Err(_) => {
                                    warn!(
                                        "Client {} sent non-numeric blksize='{}', using default {}",
                                        client_addr, value, options.block_size
                                    );
                                }
                            }
                        }
                        "timeout" => {
                            // RFC 2349 - Timeout Interval Option (valid range: 1-255 seconds)
                            match value.parse::<u64>() {
                                Ok(timeout) if (1..=255).contains(&timeout) => {
                                    options.timeout = timeout;
                                    negotiated_options
                                        .insert("timeout".to_string(), timeout.to_string());
                                }
                                Ok(timeout) => {
                                    warn!(
                                        "Client {} requested invalid timeout={} (valid: 1-255), using default {}",
                                        client_addr, timeout, options.timeout
                                    );
                                }
                                Err(_) => {
                                    warn!(
                                        "Client {} sent non-numeric timeout='{}', using default {}",
                                        client_addr, value, options.timeout
                                    );
                                }
                            }
                        }
                        "tsize" => {
                            // RFC 2349 - Transfer Size Option
                            // For RRQ, client sends 0 and server responds with actual size
                            match value.parse::<u64>() {
                                Ok(0) => {
                                    negotiated_options.insert("tsize".to_string(), "0".to_string());
                                    // Will be filled with actual size later
                                }
                                Ok(size) => {
                                    // Client sent non-zero tsize for RRQ - unusual but not invalid
                                    debug!(
                                        "Client {} sent tsize={} for RRQ (expected 0), will respond with actual size",
                                        client_addr, size
                                    );
                                    negotiated_options.insert("tsize".to_string(), "0".to_string());
                                }
                                Err(_) => {
                                    warn!(
                                        "Client {} sent non-numeric tsize='{}', omitting from OACK",
                                        client_addr, value
                                    );
                                }
                            }
                        }
                        "windowsize" => {
                            // RFC 7440 - Windowsize Option (valid range: 1-65535 blocks)
                            match value.parse::<usize>() {
                                Ok(size) if (1..=65535).contains(&size) => {
                                    // Server can accept or negotiate down
                                    // For now, accept client's windowsize if valid
                                    options.windowsize = size;
                                    negotiated_options
                                        .insert("windowsize".to_string(), size.to_string());
                                }
                                Ok(size) => {
                                    warn!(
                                        "Client {} requested invalid windowsize={} (valid: 1-65535), using default {}",
                                        client_addr, size, options.windowsize
                                    );
                                }
                                Err(_) => {
                                    warn!(
                                        "Client {} sent non-numeric windowsize='{}', using default {}",
                                        client_addr, value, options.windowsize
                                    );
                                }
                            }
                        }
                        "multicast" => {
                            // RFC 2090: Multicast option (handled separately)
                            // Don't add to negotiated_options here
                        }
                        _ => {
                            // RFC 2347: Unknown options are silently ignored
                            debug!(
                                "Client {} sent unknown option '{}', ignoring per RFC 2347",
                                client_addr, name
                            );
                        }
                    }
                }

                debug!(
                    "RRQ from {}: {} (mode: {}, options: {:?}, multicast: {})",
                    client_addr, filename, mode_str, negotiated_options, multicast_requested
                );

                // Audit log: Read request received
                if audit_enabled {
                    AuditLogger::read_request(
                        client_addr,
                        &filename,
                        &mode_str,
                        serde_json::to_value(&negotiated_options).unwrap_or(serde_json::json!({})),
                    );
                }

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
                        // Audit log: Path validation failure
                        if audit_enabled {
                            if filename.contains("..") {
                                AuditLogger::path_traversal_attempt(
                                    client_addr,
                                    &filename,
                                    "directory traversal attempt",
                                );
                            } else {
                                AuditLogger::access_violation(
                                    client_addr,
                                    &filename,
                                    &e.to_string(),
                                );
                            }
                        }

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
                    audit_enabled,
                    &file_io_config,
                )
                .await?;
            }
            TftpOpcode::Wrq => {
                // RFC 1350: WRQ packet format
                // 2 bytes: opcode (02)
                // string: filename (null-terminated)
                // string: mode (null-terminated)
                // RFC 2347: followed by optional option/value pairs

                let filename = Self::parse_string(&mut bytes)?;
                let mode_str = Self::parse_string(&mut bytes)?;

                // Check if writes are enabled
                if !write_config.enabled {
                    warn!("WRQ from {}: writes disabled", client_addr);

                    // Audit log: Write request denied
                    if audit_enabled {
                        AuditLogger::write_request_denied(
                            client_addr,
                            &filename,
                            "writes disabled in configuration",
                        );
                    }

                    Self::send_error(
                        client_addr,
                        TftpErrorCode::AccessViolation,
                        "Write not supported",
                    )
                    .await?;
                    return Ok(());
                }

                // Validate transfer mode
                let mode = TransferMode::from_str(&mode_str)?;

                // RFC 1350: Reject obsolete MAIL mode
                if mode == TransferMode::Mail {
                    warn!(
                        "WRQ MAIL mode requested from {}: obsolete and not supported",
                        client_addr
                    );

                    if audit_enabled {
                        AuditLogger::write_request_denied(
                            client_addr,
                            &filename,
                            "MAIL mode not supported",
                        );
                    }

                    Self::send_error(
                        client_addr,
                        TftpErrorCode::IllegalOperation,
                        "MAIL mode not supported",
                    )
                    .await?;
                    return Ok(());
                }

                // Parse options (RFC 2347)
                let mut options = TftpOptions {
                    windowsize: default_windowsize,
                    ..TftpOptions::default()
                };
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

                // RFC 2347: Option negotiation
                // Server MUST either accept option with valid value or omit from OACK
                let mut negotiated_options = HashMap::new();

                for (name, value) in &requested_options {
                    match name.as_str() {
                        "blksize" => {
                            // RFC 2348 - Block Size Option (valid range: 8-65464 bytes)
                            match value.parse::<usize>() {
                                Ok(size) if (8..=MAX_BLOCK_SIZE).contains(&size) => {
                                    options.block_size = size;
                                    negotiated_options
                                        .insert("blksize".to_string(), size.to_string());
                                }
                                Ok(size) => {
                                    // Invalid size - log and omit from OACK per RFC 2347
                                    warn!(
                                        "Client {} requested invalid blksize={} (valid: 8-{}), using default {}",
                                        client_addr, size, MAX_BLOCK_SIZE, options.block_size
                                    );
                                }
                                Err(_) => {
                                    warn!(
                                        "Client {} sent non-numeric blksize='{}', using default {}",
                                        client_addr, value, options.block_size
                                    );
                                }
                            }
                        }
                        "timeout" => {
                            // RFC 2349 - Timeout Interval Option (valid range: 1-255 seconds)
                            match value.parse::<u64>() {
                                Ok(timeout) if (1..=255).contains(&timeout) => {
                                    options.timeout = timeout;
                                    negotiated_options
                                        .insert("timeout".to_string(), timeout.to_string());
                                }
                                Ok(timeout) => {
                                    warn!(
                                        "Client {} requested invalid timeout={} (valid: 1-255), using default {}",
                                        client_addr, timeout, options.timeout
                                    );
                                }
                                Err(_) => {
                                    warn!(
                                        "Client {} sent non-numeric timeout='{}', using default {}",
                                        client_addr, value, options.timeout
                                    );
                                }
                            }
                        }
                        "tsize" => {
                            // RFC 2349 - Transfer Size Option
                            // For WRQ, client sends expected size (may be 0 if unknown)
                            match value.parse::<u64>() {
                                Ok(size) => {
                                    options.transfer_size = Some(size);
                                    negotiated_options
                                        .insert("tsize".to_string(), size.to_string());
                                }
                                Err(_) => {
                                    warn!(
                                        "Client {} sent non-numeric tsize='{}', omitting from OACK",
                                        client_addr, value
                                    );
                                }
                            }
                        }
                        "windowsize" => {
                            // RFC 7440 - Windowsize Option (valid range: 1-65535 blocks)
                            match value.parse::<usize>() {
                                Ok(size) if (1..=65535).contains(&size) => {
                                    // Server can accept or negotiate down
                                    // For now, accept client's windowsize if valid
                                    options.windowsize = size;
                                    negotiated_options
                                        .insert("windowsize".to_string(), size.to_string());
                                }
                                Ok(size) => {
                                    warn!(
                                        "Client {} requested invalid windowsize={} (valid: 1-65535), using default {}",
                                        client_addr, size, options.windowsize
                                    );
                                }
                                Err(_) => {
                                    warn!(
                                        "Client {} sent non-numeric windowsize='{}', using default {}",
                                        client_addr, value, options.windowsize
                                    );
                                }
                            }
                        }
                        _ => {
                            // RFC 2347: Unknown options are silently ignored
                            debug!(
                                "Client {} sent unknown option '{}', ignoring per RFC 2347",
                                client_addr, name
                            );
                        }
                    }
                }

                debug!(
                    "WRQ from {}: {} (mode: {}, options: {:?})",
                    client_addr, filename, mode_str, negotiated_options
                );

                // Audit log: Write request received
                if audit_enabled {
                    AuditLogger::write_request(
                        client_addr,
                        &filename,
                        &mode_str,
                        serde_json::to_value(&negotiated_options).unwrap_or(serde_json::json!({})),
                    );
                }

                // Validate filename (prevent directory traversal)
                let file_path = match Self::validate_and_resolve_path(&root_dir, &filename) {
                    Ok(path) => path,
                    Err(e) => {
                        // Audit log: Path validation failure
                        if audit_enabled {
                            if filename.contains("..") {
                                AuditLogger::path_traversal_attempt(
                                    client_addr,
                                    &filename,
                                    "directory traversal attempt",
                                );
                            } else {
                                AuditLogger::access_violation(
                                    client_addr,
                                    &filename,
                                    &e.to_string(),
                                );
                            }
                        }

                        Self::send_error(
                            client_addr,
                            TftpErrorCode::AccessViolation,
                            &e.to_string(),
                        )
                        .await?;
                        return Ok(());
                    }
                };

                // Check if filename matches allowed patterns
                if !Self::is_write_allowed(&file_path, &root_dir, &write_config) {
                    warn!(
                        "WRQ from {}: {} not in allowed patterns",
                        client_addr, filename
                    );

                    if audit_enabled {
                        AuditLogger::write_request_denied(
                            client_addr,
                            &filename,
                            "file not in allowed_patterns",
                        );
                    }

                    Self::send_error(
                        client_addr,
                        TftpErrorCode::AccessViolation,
                        "File not allowed for writing",
                    )
                    .await?;
                    return Ok(());
                }

                // Check if file exists
                let file_exists = file_path.exists();

                if file_exists && !write_config.allow_overwrite {
                    // RFC 1350: File already exists error
                    warn!(
                        "WRQ from {}: file exists and overwrite disabled",
                        client_addr
                    );

                    if audit_enabled {
                        AuditLogger::write_request_denied(
                            client_addr,
                            &filename,
                            "file exists and overwrite disabled",
                        );
                    }

                    Self::send_error(
                        client_addr,
                        TftpErrorCode::FileExists,
                        "File already exists",
                    )
                    .await?;
                    return Ok(());
                }

                Self::handle_write_request(
                    file_path,
                    client_addr,
                    mode,
                    options,
                    negotiated_options,
                    max_file_size_bytes,
                    !file_exists,
                    audit_enabled,
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
    #[allow(clippy::too_many_arguments)]
    async fn handle_read_request(
        file_path: PathBuf,
        client_addr: SocketAddr,
        mode: TransferMode,
        options: TftpOptions,
        mut negotiated_options: HashMap<String, String>,
        max_file_size_bytes: u64,
        audit_enabled: bool,
        file_io_config: &config::FileIoConfig,
    ) -> Result<()> {
        let start_time = std::time::Instant::now();
        // RFC 1350: Each transfer connection uses a new TID (Transfer ID)
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        socket.connect(client_addr).await?;

        // Open and validate file
        let mut file = match File::open(&file_path).await {
            Ok(f) => f,
            Err(_) => {
                // Audit log: File not found
                if audit_enabled {
                    AuditLogger::read_denied(
                        client_addr,
                        &file_path.display().to_string(),
                        "File not found",
                    );
                }

                Self::send_error_on_socket(&socket, TftpErrorCode::FileNotFound, "File not found")
                    .await?;
                return Ok(());
            }
        };

        let file_metadata = file.metadata().await?;
        let file_size = file_metadata.len();

        // Phase 1: Apply file I/O hints for optimal kernel behavior
        apply_file_hints(&file, file_io_config, file_size);

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

            // Audit log: File size limit exceeded
            if audit_enabled {
                AuditLogger::file_size_limit_exceeded(
                    client_addr,
                    &file_path.display().to_string(),
                    file_size,
                    max_file_size_bytes,
                );
            }

            Self::send_error_on_socket(&socket, TftpErrorCode::DiskFull, "File too large").await?;
            return Ok(());
        }

        // Audit log: Transfer started
        if audit_enabled {
            let mode_str = match mode {
                TransferMode::Netascii => "netascii",
                TransferMode::Octet => "octet",
                TransferMode::Mail => "mail",
            };
            AuditLogger::transfer_started(
                client_addr,
                &file_path.display().to_string(),
                file_size,
                mode_str,
                options.block_size,
            );
        }

        let block_size = options.block_size;
        let timeout = tokio::time::Duration::from_secs(options.timeout);

        // For NETASCII mode with small files, use full buffering for line ending conversion
        // For OCTET mode or larger files, use streaming approach
        // Performance optimization: Stream files directly without full buffering
        if mode == TransferMode::Netascii && file_size <= 1_048_576 {
            // Small NETASCII files (<1MB) - use full buffering for line ending conversion
            let mut raw_data = Vec::new();
            file.read_to_end(&mut raw_data).await?;
            let file_data = TransferMode::convert_to_netascii(&raw_data);

            // RFC 2349: Update tsize with converted size
            if negotiated_options.contains_key("tsize") {
                negotiated_options.insert("tsize".to_string(), file_data.len().to_string());
            }

            // RFC 2347: Send OACK if options were negotiated
            if !negotiated_options.is_empty() {
                debug!("Sending OACK with options: {:?}", negotiated_options);
                let oack_packet = Self::build_oack_packet(&negotiated_options);
                Self::send_with_retry(&socket, &oack_packet, timeout).await?;
                match Self::wait_for_ack(&socket, 0, timeout).await {
                    Ok(()) => {}
                    Err(e) => {
                        error!("Failed to receive ACK for OACK: {}", e);
                        return Ok(());
                    }
                }
            }

            Self::send_file_data_buffered(
                &socket,
                &file_data,
                block_size,
                options.windowsize,
                timeout,
                client_addr,
                &file_path,
                start_time,
                audit_enabled,
            )
            .await
        } else {
            // Large files or OCTET mode - use streaming approach
            // RFC 2349: Update tsize with file size
            if negotiated_options.contains_key("tsize") {
                negotiated_options.insert("tsize".to_string(), file_size.to_string());
            }

            // RFC 2347: Send OACK if options were negotiated
            if !negotiated_options.is_empty() {
                debug!("Sending OACK with options: {:?}", negotiated_options);
                let oack_packet = Self::build_oack_packet(&negotiated_options);
                Self::send_with_retry(&socket, &oack_packet, timeout).await?;
                match Self::wait_for_ack(&socket, 0, timeout).await {
                    Ok(()) => {}
                    Err(e) => {
                        error!("Failed to receive ACK for OACK: {}", e);
                        return Ok(());
                    }
                }
            }

            Self::send_file_data_streaming(
                &socket,
                file,
                file_size,
                mode,
                block_size,
                options.windowsize,
                timeout,
                client_addr,
                &file_path,
                start_time,
                audit_enabled,
            )
            .await
        }
    }

    /// Send file data using buffered approach (for small NETASCII files)
    /// RFC 7440: Supports windowsize for sending multiple blocks before ACK
    #[allow(clippy::too_many_arguments)]
    async fn send_file_data_buffered(
        socket: &UdpSocket,
        file_data: &[u8],
        block_size: usize,
        windowsize: usize,
        timeout: tokio::time::Duration,
        client_addr: SocketAddr,
        file_path: &Path,
        start_time: std::time::Instant,
        audit_enabled: bool,
    ) -> Result<()> {
        if file_data.is_empty() {
            // Send a single empty data block
            let mut data_packet = BytesMut::with_capacity(4);
            data_packet.put_u16(TftpOpcode::Data as u16);
            data_packet.put_u16(1);

            Self::send_with_retry(socket, &data_packet, timeout).await?;
            Self::wait_for_ack(socket, 1, timeout).await?;

            debug!("Transfer complete: empty file");

            if audit_enabled {
                let duration_ms = start_time.elapsed().as_millis() as u64;
                AuditLogger::transfer_completed(
                    client_addr,
                    &file_path.display().to_string(),
                    0,
                    1,
                    duration_ms,
                );
            }
            return Ok(());
        }

        let mut block_num: u16 = 1;
        let mut offset = 0;

        // RFC 7440: Sliding window transmission
        // Send windowsize blocks, then wait for ACK of the last block
        while offset < file_data.len() {
            let window_start_block = block_num;
            let mut window_packets = Vec::with_capacity(windowsize);
            let mut blocks_in_window = 0;
            let mut temp_offset = offset;
            let mut temp_block_num = block_num;

            // Build a window of packets
            while blocks_in_window < windowsize && temp_offset < file_data.len() {
                let bytes_to_send = std::cmp::min(block_size, file_data.len() - temp_offset);
                let block_data = &file_data[temp_offset..temp_offset + bytes_to_send];

                let mut data_packet = BytesMut::with_capacity(4 + bytes_to_send);
                data_packet.put_u16(TftpOpcode::Data as u16);
                data_packet.put_u16(temp_block_num);
                data_packet.put_slice(block_data);

                window_packets.push((temp_block_num, data_packet.freeze(), bytes_to_send));

                temp_offset += bytes_to_send;
                temp_block_num = temp_block_num.wrapping_add(1);
                blocks_in_window += 1;

                // Stop if this is the last block (less than block_size)
                if bytes_to_send < block_size {
                    break;
                }
            }

            // Send all blocks in the window
            let mut retries = 0;
            let last_block_in_window = window_packets.last().unwrap().0;

            loop {
                if retries >= MAX_RETRIES {
                    error!(
                        "Max retries exceeded for window starting at block {} after {} attempts",
                        window_start_block, MAX_RETRIES
                    );
                    return Ok(());
                }

                // Send all packets in window
                for (_, packet, _) in &window_packets {
                    socket.send(packet).await?;
                }

                // RFC 7440: Wait for ACK of the last block in the window
                match Self::wait_for_ack_with_duplicate_handling(
                    socket,
                    last_block_in_window,
                    timeout,
                    &window_packets.last().unwrap().1,
                )
                .await
                {
                    Ok(true) => break,
                    Ok(false) => {
                        debug!(
                            "Duplicate or out-of-order ACK for window ending at block {}, retransmitting window",
                            last_block_in_window
                        );
                        retries += 1;
                        continue;
                    }
                    Err(e) => {
                        debug!(
                            "Timeout or error waiting for ACK of block {}: {}, retransmitting window",
                            last_block_in_window, e
                        );
                        retries += 1;
                        continue;
                    }
                }
            }

            // Move forward by the number of blocks sent
            for (blk_num, _, bytes_sent) in &window_packets {
                offset += bytes_sent;
                block_num = blk_num.wrapping_add(1);

                // Check if this was the final block
                if *bytes_sent < block_size {
                    debug!(
                        "Transfer complete: {} blocks sent ({} bytes)",
                        blk_num,
                        file_data.len()
                    );
                    if audit_enabled {
                        let duration_ms = start_time.elapsed().as_millis() as u64;
                        AuditLogger::transfer_completed(
                            client_addr,
                            &file_path.display().to_string(),
                            file_data.len() as u64,
                            *blk_num,
                            duration_ms,
                        );
                    }
                    return Ok(());
                }
            }
        }

        Ok(())
    }

    /// Send file data using streaming approach (for large files and OCTET mode)
    /// Performance optimization: Reads file in chunks to minimize memory usage
    #[allow(clippy::too_many_arguments)]
    /// Send file data using streaming approach (for large files and OCTET mode)
    /// RFC 7440: Supports windowsize for sending multiple blocks before ACK
    #[allow(clippy::too_many_arguments)]
    async fn send_file_data_streaming(
        socket: &UdpSocket,
        mut file: File,
        file_size: u64,
        mode: TransferMode,
        block_size: usize,
        windowsize: usize,
        timeout: tokio::time::Duration,
        client_addr: SocketAddr,
        file_path: &Path,
        start_time: std::time::Instant,
        audit_enabled: bool,
    ) -> Result<()> {
        if file_size == 0 {
            // Send a single empty data block
            let mut data_packet = BytesMut::with_capacity(4);
            data_packet.put_u16(TftpOpcode::Data as u16);
            data_packet.put_u16(1);

            Self::send_with_retry(socket, &data_packet, timeout).await?;
            Self::wait_for_ack(socket, 1, timeout).await?;

            debug!("Transfer complete: empty file (streaming mode)");

            if audit_enabled {
                let duration_ms = start_time.elapsed().as_millis() as u64;
                AuditLogger::transfer_completed(
                    client_addr,
                    &file_path.display().to_string(),
                    0,
                    1,
                    duration_ms,
                );
            }
            return Ok(());
        }

        let mut block_num: u16 = 1;
        let mut bytes_transferred: u64 = 0;
        let mut read_buffer = vec![0u8; block_size];
        let mut netascii_buffer = Vec::new();
        let mut eof_reached = false;

        // RFC 7440: Sliding window transmission for streaming
        loop {
            let mut window_packets = Vec::with_capacity(windowsize);
            let mut blocks_in_window = 0;
            let window_start_block = block_num;

            // Build a window of packets by reading from file
            while blocks_in_window < windowsize && !eof_reached {
                let bytes_read = file.read(&mut read_buffer).await?;

                if bytes_read == 0 {
                    eof_reached = true;
                    break;
                }

                // Determine block data based on mode
                let block_data = if mode == TransferMode::Netascii {
                    netascii_buffer.clear();
                    netascii_buffer.extend_from_slice(
                        TransferMode::convert_to_netascii(&read_buffer[..bytes_read]).as_slice(),
                    );
                    netascii_buffer.clone()
                } else {
                    read_buffer[..bytes_read].to_vec()
                };

                let mut data_packet = BytesMut::with_capacity(4 + block_data.len());
                data_packet.put_u16(TftpOpcode::Data as u16);
                data_packet.put_u16(block_num);
                data_packet.put_slice(&block_data);

                let is_final = bytes_read < block_size;
                window_packets.push((block_num, data_packet.freeze(), block_data.len(), is_final));

                block_num = block_num.wrapping_add(1);
                blocks_in_window += 1;

                if is_final {
                    eof_reached = true;
                    break;
                }
            }

            // If no packets in window, we're done
            if window_packets.is_empty() {
                break;
            }

            // Send all blocks in the window with retry
            let mut retries = 0;
            let last_block_in_window = window_packets.last().unwrap().0;

            loop {
                if retries >= MAX_RETRIES {
                    error!(
                        "Max retries exceeded for window starting at block {} after {} attempts",
                        window_start_block, MAX_RETRIES
                    );
                    return Ok(());
                }

                // Send all packets in window
                for (_, packet, _, _) in &window_packets {
                    socket.send(packet).await?;
                }

                // RFC 7440: Wait for ACK of the last block in the window
                match Self::wait_for_ack_with_duplicate_handling(
                    socket,
                    last_block_in_window,
                    timeout,
                    &window_packets.last().unwrap().1,
                )
                .await
                {
                    Ok(true) => break,
                    Ok(false) => {
                        debug!(
                            "Duplicate or out-of-order ACK for window ending at block {}, retransmitting window",
                            last_block_in_window
                        );
                        retries += 1;
                        continue;
                    }
                    Err(e) => {
                        debug!(
                            "Timeout waiting for ACK of block {}: {}, retransmitting window",
                            last_block_in_window, e
                        );
                        retries += 1;
                        continue;
                    }
                }
            }

            // Update bytes transferred and check for completion
            for (blk_num, _, bytes_sent, is_final) in &window_packets {
                bytes_transferred += *bytes_sent as u64;

                if *is_final {
                    debug!(
                        "Transfer complete: {} blocks sent ({} bytes, streaming mode)",
                        blk_num, bytes_transferred
                    );
                    if audit_enabled {
                        let duration_ms = start_time.elapsed().as_millis() as u64;
                        AuditLogger::transfer_completed(
                            client_addr,
                            &file_path.display().to_string(),
                            bytes_transferred,
                            *blk_num,
                            duration_ms,
                        );
                    }
                    return Ok(());
                }
            }

            if eof_reached {
                break;
            }
        }

        Ok(())
    }

    /// Handle WRQ (Write Request) with support for NETASCII and OCTET modes
    ///
    /// NIST Controls:
    /// - AC-3: Access Enforcement (write access validation)
    /// - SI-10: Information Input Validation (transfer mode handling, data validation)
    /// - SC-4: Information in Shared Resources (data format conversion)
    /// - AU-2: Audit Events (log all write operations)
    #[allow(clippy::too_many_arguments)]
    async fn handle_write_request(
        file_path: PathBuf,
        client_addr: SocketAddr,
        mode: TransferMode,
        options: TftpOptions,
        negotiated_options: HashMap<String, String>,
        max_file_size_bytes: u64,
        file_created: bool,
        audit_enabled: bool,
    ) -> Result<()> {
        let start_time = std::time::Instant::now();

        // RFC 1350: Each transfer connection uses a new TID (Transfer ID)
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        socket.connect(client_addr).await?;

        // Audit log: Write started
        if audit_enabled {
            let mode_str = match mode {
                TransferMode::Netascii => "netascii",
                TransferMode::Octet => "octet",
                TransferMode::Mail => "mail",
            };
            AuditLogger::write_started(
                client_addr,
                &file_path.display().to_string(),
                mode_str,
                options.block_size,
            );
        }

        let block_size = options.block_size;
        let windowsize = options.windowsize;
        let timeout = tokio::time::Duration::from_secs(options.timeout);

        // RFC 2347: Send OACK if options were negotiated, or ACK block 0 to begin transfer
        if !negotiated_options.is_empty() {
            debug!("Sending OACK with options: {:?}", negotiated_options);

            let oack_packet = Self::build_oack_packet(&negotiated_options);
            Self::send_with_retry(&socket, &oack_packet, timeout).await?;
        } else {
            // No options - send ACK of block 0 to signal ready to receive
            let mut ack_packet = BytesMut::with_capacity(4);
            ack_packet.put_u16(TftpOpcode::Ack as u16);
            ack_packet.put_u16(0);
            Self::send_with_retry(&socket, &ack_packet, timeout).await?;
        }

        // Receive file data blocks
        // Performance optimization: Pre-allocate buffer with expected size if available
        let mut received_data = if let Some(expected_size) = options.transfer_size {
            Vec::with_capacity(expected_size as usize)
        } else {
            // Default pre-allocation for 1MB
            Vec::with_capacity(1_048_576)
        };
        let mut expected_block: u16 = 1;
        let mut buf = vec![0u8; MAX_PACKET_SIZE];

        loop {
            // Wait for DATA packet
            match tokio::time::timeout(timeout, socket.recv(&mut buf)).await {
                Ok(Ok(size)) => {
                    if size < 4 {
                        warn!("Received invalid DATA packet (too small)");
                        continue;
                    }

                    let mut data_bytes = BytesMut::from(&buf[..size]);
                    let opcode = data_bytes.get_u16();

                    // Check for ERROR packet from client
                    if opcode == TftpOpcode::Error as u16 {
                        let error_code = data_bytes.get_u16();
                        let error_msg = Self::parse_string(&mut data_bytes).unwrap_or_default();

                        if audit_enabled {
                            AuditLogger::write_failed(
                                client_addr,
                                &file_path.display().to_string(),
                                &format!("Client sent error {}: {}", error_code, error_msg),
                                expected_block.wrapping_sub(1),
                            );
                        }

                        return Err(TftpError::Tftp(format!(
                            "Client sent error {}: {}",
                            error_code, error_msg
                        )));
                    }

                    if opcode != TftpOpcode::Data as u16 {
                        warn!("Expected DATA, got opcode {}", opcode);
                        continue;
                    }

                    let block_num = data_bytes.get_u16();

                    // Handle block number
                    if block_num < expected_block {
                        // Duplicate block - re-send ACK
                        debug!("Received duplicate block {}", block_num);
                        let mut ack_packet = BytesMut::with_capacity(4);
                        ack_packet.put_u16(TftpOpcode::Ack as u16);
                        ack_packet.put_u16(block_num);
                        socket.send(&ack_packet).await?;
                        continue;
                    } else if block_num > expected_block {
                        // Out of order - error
                        warn!(
                            "Block mismatch: expected {}, got {}",
                            expected_block, block_num
                        );
                        Self::send_error_on_socket(
                            &socket,
                            TftpErrorCode::IllegalOperation,
                            "Out of order block",
                        )
                        .await?;
                        return Ok(());
                    }

                    // Get data from packet
                    let block_data = &data_bytes[..];
                    let data_len = block_data.len();

                    // Security: Check cumulative file size
                    // NIST SC-5: Denial of Service Protection
                    if max_file_size_bytes > 0
                        && (received_data.len() + data_len) > max_file_size_bytes as usize
                    {
                        error!(
                            "Write exceeds maximum file size {} for {}",
                            max_file_size_bytes,
                            file_path.display()
                        );

                        if audit_enabled {
                            AuditLogger::file_size_limit_exceeded(
                                client_addr,
                                &file_path.display().to_string(),
                                (received_data.len() + data_len) as u64,
                                max_file_size_bytes,
                            );
                        }

                        Self::send_error_on_socket(
                            &socket,
                            TftpErrorCode::DiskFull,
                            "File too large",
                        )
                        .await?;
                        return Ok(());
                    }

                    // Append data to buffer
                    received_data.extend_from_slice(block_data);

                    // RFC 7440: Only send ACK when we've received:
                    // 1. The last block in a window, OR
                    // 2. The final block (< block_size)
                    let is_final_block = data_len < block_size;
                    let blocks_in_current_window = (block_num - 1) % windowsize as u16 + 1;
                    let should_ack =
                        blocks_in_current_window == windowsize as u16 || is_final_block;

                    if should_ack {
                        // Send ACK for the last block in window
                        let mut ack_packet = BytesMut::with_capacity(4);
                        ack_packet.put_u16(TftpOpcode::Ack as u16);
                        ack_packet.put_u16(block_num);
                        socket.send(&ack_packet).await?;

                        debug!(
                            "Received block {} (ACK sent, {} bytes, total: {} bytes)",
                            block_num,
                            data_len,
                            received_data.len()
                        );
                    } else {
                        debug!(
                            "Received block {} (buffered in window, {} bytes, total: {} bytes)",
                            block_num,
                            data_len,
                            received_data.len()
                        );
                    }

                    // RFC 1350: Transfer complete when data packet < block_size
                    if is_final_block {
                        info!(
                            "Write complete: {} blocks received ({} bytes)",
                            block_num,
                            received_data.len()
                        );
                        break;
                    }

                    expected_block = expected_block.wrapping_add(1);
                }
                Ok(Err(e)) => {
                    error!("Error receiving DATA: {}", e);

                    if audit_enabled {
                        AuditLogger::write_failed(
                            client_addr,
                            &file_path.display().to_string(),
                            &e.to_string(),
                            expected_block.wrapping_sub(1),
                        );
                    }

                    return Err(e.into());
                }
                Err(_) => {
                    error!("Timeout waiting for DATA block {}", expected_block);

                    if audit_enabled {
                        AuditLogger::write_failed(
                            client_addr,
                            &file_path.display().to_string(),
                            "timeout waiting for data",
                            expected_block.wrapping_sub(1),
                        );
                    }

                    // RFC 2349: Send ERROR packet to client on timeout
                    Self::send_error_on_socket(
                        &socket,
                        TftpErrorCode::NotDefined,
                        &format!("Timeout waiting for block {}", expected_block),
                    )
                    .await
                    .ok(); // Ignore send errors

                    return Err(TftpError::Tftp(format!(
                        "Timeout waiting for DATA block {}",
                        expected_block
                    )));
                }
            }
        }

        // Convert data if NETASCII mode
        let final_data = if mode == TransferMode::Netascii {
            // RFC 1350: NETASCII mode - convert CR+LF to local line endings (LF on Unix)
            Self::convert_from_netascii(&received_data)
        } else {
            received_data
        };

        // RFC 2349: Validate transfer size if client specified expected size
        // Check if actual received size matches the tsize option (if provided and non-zero)
        if let Some(expected_size) = options.transfer_size
            && expected_size > 0
            && final_data.len() as u64 != expected_size
        {
            warn!(
                "Transfer size mismatch: expected {} bytes, received {} bytes",
                expected_size,
                final_data.len()
            );

            if audit_enabled {
                AuditLogger::write_failed(
                    client_addr,
                    &file_path.display().to_string(),
                    &format!(
                        "Transfer size mismatch: expected {}, got {}",
                        expected_size,
                        final_data.len()
                    ),
                    expected_block,
                );
            }

            // Note: RFC 2349 doesn't specify error behavior for size mismatch
            // We log a warning but still write the file since data was transferred successfully
            debug!(
                "Proceeding with write despite size mismatch (expected: {}, actual: {})",
                expected_size,
                final_data.len()
            );
        }

        // Write file to disk
        match Self::write_file_safely(&file_path, &final_data).await {
            Ok(()) => {
                debug!(
                    "File written successfully: {} ({} bytes)",
                    file_path.display(),
                    final_data.len()
                );

                // Audit log: Write completed
                if audit_enabled {
                    let duration_ms = start_time.elapsed().as_millis() as u64;
                    AuditLogger::write_completed(
                        client_addr,
                        &file_path.display().to_string(),
                        final_data.len() as u64,
                        expected_block,
                        duration_ms,
                        file_created,
                    );
                }
            }
            Err(e) => {
                error!("Failed to write file {}: {}", file_path.display(), e);

                if audit_enabled {
                    AuditLogger::write_failed(
                        client_addr,
                        &file_path.display().to_string(),
                        &e.to_string(),
                        expected_block,
                    );
                }

                Self::send_error_on_socket(&socket, TftpErrorCode::DiskFull, "Write failed")
                    .await?;
                return Err(e);
            }
        }

        Ok(())
    }

    /// Convert data from NETASCII format (CR+LF -> LF)
    ///
    /// RFC 1350 NETASCII Specification:
    /// - Lines are terminated with CR+LF (0x0D 0x0A)
    /// - Converts network standard (CR+LF) to Unix line endings (LF)
    fn convert_from_netascii(data: &[u8]) -> Vec<u8> {
        let mut result = Vec::with_capacity(data.len());
        let mut i = 0;

        while i < data.len() {
            let byte = data[i];

            if byte == b'\r' && i + 1 < data.len() && data[i + 1] == b'\n' {
                // CR+LF sequence - convert to LF
                result.push(b'\n');
                i += 2;
            } else if byte == b'\r' {
                // Bare CR - convert to LF
                result.push(b'\n');
                i += 1;
            } else {
                // Regular character - copy as-is
                result.push(byte);
                i += 1;
            }
        }

        result
    }

    /// Write file with atomic operations to prevent partial writes
    ///
    /// NIST 800-53 Controls:
    /// - SI-7: Software, Firmware, and Information Integrity (atomic writes)
    /// - CM-5: Access Restrictions for Change (safe file modification)
    async fn write_file_safely(file_path: &Path, data: &[u8]) -> Result<()> {
        // Create parent directory if needed
        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Write to temporary file first, then rename for atomicity
        let temp_path = file_path.with_extension(".tftp-tmp");

        // Write data to temp file
        let mut file = tokio::fs::File::create(&temp_path).await?;
        file.write_all(data).await?;
        file.flush().await?;
        drop(file);

        // Atomic rename
        tokio::fs::rename(&temp_path, file_path).await?;

        Ok(())
    }

    /// Check if a file path is allowed for writing based on configured patterns
    ///
    /// NIST 800-53 Controls:
    /// - AC-3: Access Enforcement (pattern-based access control)
    /// - AC-6: Least Privilege (minimal write permissions)
    ///
    /// STIG V-222602: Applications must enforce access restrictions
    fn is_write_allowed(file_path: &Path, root_dir: &Path, write_config: &WriteConfig) -> bool {
        // Get the relative path from root_dir
        let relative_path = match file_path.strip_prefix(root_dir) {
            Ok(p) => p,
            Err(_) => return false,
        };

        // Convert to string for pattern matching
        let path_str = match relative_path.to_str() {
            Some(s) => s,
            None => return false,
        };

        // Check against all allowed patterns
        for pattern in &write_config.allowed_patterns {
            // Use glob pattern matching
            if let Ok(glob_pattern) = glob::Pattern::new(pattern)
                && glob_pattern.matches(path_str)
            {
                return true;
            }
        }

        false
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

    /// Wait for ACK with duplicate ACK detection for retransmission
    ///
    /// Returns: Ok(true) if correct ACK received, Ok(false) if duplicate ACK (should retransmit)
    ///
    /// RFC 1350: When duplicate ACK is received, retransmit the current DATA packet
    async fn wait_for_ack_with_duplicate_handling(
        socket: &UdpSocket,
        expected_block: u16,
        timeout: tokio::time::Duration,
        _data_packet: &[u8],
    ) -> Result<bool> {
        // Performance optimization: ACK packets are exactly 4 bytes, no need for 1KB buffer
        let mut ack_buf = [0u8; 16]; // Small buffer, ACKs are 4 bytes (opcode + block number)

        match tokio::time::timeout(timeout, socket.recv(&mut ack_buf)).await {
            Ok(Ok(size)) => {
                if size < 4 {
                    warn!("Received invalid ACK packet (too small)");
                    return Err(TftpError::Tftp("Invalid ACK packet".to_string()));
                }

                let mut ack_bytes = BytesMut::from(&ack_buf[..size]);
                let opcode = ack_bytes.get_u16();

                // Check for ERROR packet
                if opcode == TftpOpcode::Error as u16 {
                    let error_code = ack_bytes.get_u16();
                    let error_msg = Self::parse_string(&mut ack_bytes).unwrap_or_default();
                    return Err(TftpError::Tftp(format!(
                        "Client sent error {}: {}",
                        error_code, error_msg
                    )));
                }

                if opcode != TftpOpcode::Ack as u16 {
                    warn!("Expected ACK, got opcode {}", opcode);
                    return Err(TftpError::Tftp("Unexpected opcode".to_string()));
                }

                let ack_block = ack_bytes.get_u16();

                // RFC 1350: Check ACK block number
                if ack_block == expected_block {
                    // Correct ACK
                    Ok(true)
                } else if ack_block < expected_block {
                    // Duplicate ACK - indicates packet loss, retransmit
                    debug!(
                        "Received duplicate ACK for block {} (expected {})",
                        ack_block, expected_block
                    );
                    Ok(false)
                } else {
                    warn!(
                        "ACK mismatch: expected {}, got {}",
                        expected_block, ack_block
                    );
                    Err(TftpError::Tftp("ACK out of sequence".to_string()))
                }
            }
            Ok(Err(e)) => {
                error!("Error receiving ACK: {}", e);
                Err(e.into())
            }
            Err(_) => Err(TftpError::Tftp(format!(
                "Timeout waiting for ACK of block {}",
                expected_block
            ))),
        }
    }

    // Wait for ACK packet
    async fn wait_for_ack(
        socket: &UdpSocket,
        expected_block: u16,
        timeout: tokio::time::Duration,
    ) -> Result<()> {
        // Performance optimization: ACK packets are exactly 4 bytes
        let mut ack_buf = [0u8; 16]; // Small buffer, ACKs are 4 bytes

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
                        let error_msg: String =
                            Self::parse_string(&mut ack_bytes).unwrap_or_default();
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
                && !canonical_parent.starts_with(&canonical_root)
            {
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

    // Initialize logging with JSON support for SIEM integration
    // NIST 800-53 AU-9: Protection of Audit Information
    // NIST 800-53 AU-12: Audit Generation
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

        match config.logging.format {
            LogFormat::Json => {
                tracing_subscriber::fmt()
                    .json()
                    .with_env_filter(EnvFilter::new(config.logging.level.clone()))
                    .with_writer(non_blocking)
                    .init();
            }
            LogFormat::Text => {
                tracing_subscriber::fmt()
                    .with_env_filter(EnvFilter::new(config.logging.level.clone()))
                    .with_writer(non_blocking)
                    .init();
            }
        }

        Some(guard)
    } else {
        match config.logging.format {
            LogFormat::Json => {
                tracing_subscriber::fmt()
                    .json()
                    .with_env_filter(EnvFilter::new(config.logging.level.clone()))
                    .init();
            }
            LogFormat::Text => {
                tracing_subscriber::fmt()
                    .with_env_filter(EnvFilter::new(config.logging.level.clone()))
                    .init();
            }
        }

        None
    };

    // Audit log: Server startup
    if config.logging.audit_enabled {
        AuditLogger::server_started(
            &config.bind_addr.to_string(),
            &config.root_dir.display().to_string(),
            config.multicast.enabled,
        );
    }

    let config_arc = Arc::new(config);
    let server = TftpServer::new(
        config_arc.root_dir.clone(),
        config_arc.bind_addr,
        config_arc.max_file_size_bytes,
        config_arc.write_config.clone(),
        config_arc.logging.audit_enabled,
        config_arc.clone(),
    )
    .with_multicast(config_arc.multicast.clone());
    server.run().await
}
