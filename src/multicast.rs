use bytes::{BufMut, BytesMut};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use tokio::time::{Duration, timeout};
use tracing::{debug, error, info, warn};

use crate::audit::AuditLogger;
use crate::config::MulticastConfig;
use crate::error::{Result, TftpError};
use crate::{DEFAULT_BLOCK_SIZE, TftpOpcode, TftpOptions, TransferMode};

/// RFC 2090 - TFTP Multicast Option Extension
///
/// This module implements experimental multicast TFTP support for efficient
/// simultaneous deployment to multiple clients.
///
/// Key Features:
/// - Master client election for coordinated transfers
/// - Per-client ACK tracking and state management
/// - Selective retransmission for missed blocks
/// - Multicast group management
///
/// NIST Controls:
/// - SC-5: Denial of Service Protection (efficient bandwidth usage)
/// - AC-3: Access Enforcement (session isolation)
/// - AU-2: Audit Events (session and transfer logging)
const MULTICAST_OPTION: &str = "multicast";

/// Client state in a multicast session
///
/// NIST Controls:
/// - AU-3: Content of Audit Records (track client participation)
/// - SC-5(2): Capacity, Bandwidth, and Redundancy (per-client tracking)
#[derive(Debug, Clone)]
struct ClientState {
    addr: SocketAddr,
    /// Set of block numbers this client has acknowledged
    acked_blocks: HashSet<u16>,
    /// Last activity timestamp
    last_seen: std::time::Instant,
    /// Whether this client is the master client
    is_master: bool,
}

impl ClientState {
    fn new(addr: SocketAddr, is_master: bool) -> Self {
        Self {
            addr,
            acked_blocks: HashSet::new(),
            last_seen: std::time::Instant::now(),
            is_master,
        }
    }

    #[allow(dead_code)]
    fn mark_acked(&mut self, block_num: u16) {
        self.acked_blocks.insert(block_num);
        self.last_seen = std::time::Instant::now();
    }

    fn has_acked(&self, block_num: u16) -> bool {
        self.acked_blocks.contains(&block_num)
    }
}

/// Multicast session state
///
/// RFC 2090: Manages a group of clients receiving the same file
///
/// NIST Controls:
/// - AC-3: Access Enforcement (session membership control)
/// - SC-5: Denial of Service Protection (resource limits)
/// - AU-2: Audit Events (session lifecycle tracking)
#[derive(Debug)]
pub struct MulticastSession {
    /// Session ID (for logging)
    session_id: String,
    /// File being transferred
    file_path: PathBuf,
    /// Transfer mode
    mode: TransferMode,
    /// TFTP options
    options: TftpOptions,
    /// Multicast group address
    multicast_addr: IpAddr,
    /// Multicast port
    multicast_port: u16,
    /// Map of client addresses to their state
    clients: HashMap<SocketAddr, ClientState>,
    /// Maximum clients allowed
    max_clients: usize,
    /// Master client address
    master_client: Option<SocketAddr>,
    /// Total number of blocks in file
    #[allow(dead_code)]
    total_blocks: u16,
    /// Blocks that need retransmission
    retransmit_queue: HashSet<u16>,
}

impl MulticastSession {
    /// Create a new multicast session
    ///
    /// NIST Controls:
    /// - SC-5: Denial of Service Protection (session limits)
    /// - AU-2: Audit Events (session creation logging)
    pub fn new(
        file_path: PathBuf,
        mode: TransferMode,
        options: TftpOptions,
        multicast_addr: IpAddr,
        multicast_port: u16,
        max_clients: usize,
    ) -> Self {
        let session_id = uuid::Uuid::new_v4().to_string();
        info!(
            "Creating multicast session {} for file {:?} ({}:{}, max clients: {})",
            session_id, file_path, multicast_addr, multicast_port, max_clients
        );

        Self {
            session_id,
            file_path,
            mode,
            options,
            multicast_addr,
            multicast_port,
            clients: HashMap::new(),
            max_clients,
            master_client: None,
            total_blocks: 0,
            retransmit_queue: HashSet::new(),
        }
    }

    /// Add a client to the session
    ///
    /// RFC 2090: Master client is the first client to join
    ///
    /// NIST Controls:
    /// - AC-3: Access Enforcement (client admission control)
    /// - SC-5: Denial of Service Protection (max clients limit)
    /// - AU-2: Audit Events (client join logging)
    pub fn add_client(&mut self, addr: SocketAddr) -> Result<bool> {
        // NIST SC-5: Enforce maximum client limit
        if self.clients.len() >= self.max_clients {
            warn!(
                "Session {} rejected client {}: max clients ({}) reached",
                self.session_id, addr, self.max_clients
            );
            return Err(TftpError::Tftp(format!(
                "Maximum clients ({}) reached",
                self.max_clients
            )));
        }

        // RFC 2090: First client becomes master
        let is_master = self.clients.is_empty();
        if is_master {
            self.master_client = Some(addr);
            info!(
                "Session {}: client {} elected as master",
                self.session_id, addr
            );
        }

        self.clients.insert(addr, ClientState::new(addr, is_master));
        info!(
            "Session {}: added client {} ({}/{} clients)",
            self.session_id,
            addr,
            self.clients.len(),
            self.max_clients
        );

        Ok(is_master)
    }

    /// Record ACK from a client
    ///
    /// RFC 2090: Track which blocks each client has received
    ///
    /// NIST Controls:
    /// - AU-3: Content of Audit Records (ACK tracking)
    /// - SC-5(2): Capacity, Bandwidth, and Redundancy (per-client state)
    pub fn record_ack(&mut self, addr: SocketAddr, block_num: u16) {
        if let Some(client) = self.clients.get_mut(&addr) {
            client.mark_acked(block_num);
            debug!(
                "Session {}: client {} ACKed block {}",
                self.session_id, addr, block_num
            );
        }
    }

    /// Check if all clients have acknowledged a block
    ///
    /// RFC 2090: Only proceed to next block when all clients confirm
    ///
    /// NIST Controls:
    /// - SC-5(2): Capacity, Bandwidth, and Redundancy (synchronization)
    pub fn all_clients_acked(&self, block_num: u16) -> bool {
        if self.clients.is_empty() {
            return false;
        }

        self.clients
            .values()
            .all(|client| client.has_acked(block_num))
    }

    /// Get clients that haven't acknowledged a block
    ///
    /// RFC 2090: Identify clients that need retransmission
    ///
    /// NIST Controls:
    /// - SC-5(2): Capacity, Bandwidth, and Redundancy (retransmission targeting)
    pub fn get_missing_clients(&self, block_num: u16) -> Vec<SocketAddr> {
        self.clients
            .values()
            .filter(|client| !client.has_acked(block_num))
            .map(|client| client.addr)
            .collect()
    }

    /// Queue a block for retransmission
    ///
    /// RFC 2090: Selective retransmission for efficiency
    ///
    /// NIST Controls:
    /// - SC-5: Denial of Service Protection (efficient retransmission)
    pub fn queue_retransmit(&mut self, block_num: u16) {
        self.retransmit_queue.insert(block_num);
        debug!(
            "Session {}: queued block {} for retransmission",
            self.session_id, block_num
        );
    }

    /// Get and clear retransmit queue
    ///
    /// NIST Controls:
    /// - SC-5(2): Capacity, Bandwidth, and Redundancy (batch retransmission)
    pub fn take_retransmit_queue(&mut self) -> Vec<u16> {
        let queue: Vec<u16> = self.retransmit_queue.iter().copied().collect();
        self.retransmit_queue.clear();
        queue
    }

    /// Remove inactive clients
    ///
    /// RFC 2090: Handle client timeouts gracefully
    ///
    /// NIST Controls:
    /// - SC-5: Denial of Service Protection (resource cleanup)
    /// - AU-2: Audit Events (client timeout logging)
    pub fn remove_inactive_clients(&mut self, timeout_secs: u64, audit_enabled: bool) {
        let timeout_duration = std::time::Duration::from_secs(timeout_secs);
        let now = std::time::Instant::now();

        let inactive: Vec<SocketAddr> = self
            .clients
            .values()
            .filter(|client| now.duration_since(client.last_seen) > timeout_duration)
            .map(|client| client.addr)
            .collect();

        for addr in inactive {
            warn!(
                "Session {}: removing inactive client {}",
                self.session_id, addr
            );
            self.clients.remove(&addr);

            // Audit log: Client removed
            if audit_enabled {
                AuditLogger::multicast_client_removed(
                    &self.session_id,
                    addr,
                    "timeout",
                    self.clients.len(),
                );
            }

            // RFC 2090: Elect new master if master client times out
            if Some(addr) == self.master_client {
                self.elect_new_master();
            }
        }
    }

    /// Elect a new master client
    ///
    /// RFC 2090: Promote a client to master when current master leaves
    ///
    /// NIST Controls:
    /// - AC-3: Access Enforcement (master role assignment)
    /// - AU-2: Audit Events (master election logging)
    fn elect_new_master(&mut self) {
        // Clear old master
        if let Some(old_master) = self.master_client
            && let Some(client) = self.clients.get_mut(&old_master)
        {
            client.is_master = false;
        }

        // Elect first available client as new master
        if let Some((addr, _)) = self.clients.iter().next() {
            let new_master = *addr;
            self.master_client = Some(new_master);
            if let Some(client) = self.clients.get_mut(&new_master) {
                client.is_master = true;
            }
            info!(
                "Session {}: elected new master client {}",
                self.session_id, new_master
            );
        } else {
            self.master_client = None;
            info!(
                "Session {}: no clients available for master",
                self.session_id
            );
        }
    }

    /// Check if session is empty (no clients)
    pub fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }

    /// Get number of active clients
    pub fn client_count(&self) -> usize {
        self.clients.len()
    }

    /// Get session ID for audit logging
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}

/// Multicast TFTP server manager
///
/// RFC 2090: Manages multiple concurrent multicast sessions
///
/// NIST Controls:
/// - SC-5: Denial of Service Protection (session management)
/// - AC-3: Access Enforcement (session isolation)
/// - AU-2: Audit Events (comprehensive logging)
pub struct MulticastTftpServer {
    config: MulticastConfig,
    root_dir: PathBuf,
    sessions: Arc<RwLock<HashMap<String, Arc<RwLock<MulticastSession>>>>>,
    audit_enabled: bool,
}

impl MulticastTftpServer {
    /// Create a new multicast TFTP server
    ///
    /// NIST Controls:
    /// - CM-6: Configuration Settings (apply multicast configuration)
    pub fn new(config: MulticastConfig, root_dir: PathBuf, audit_enabled: bool) -> Self {
        info!(
            "Initializing multicast TFTP server ({}:{}, max clients: {})",
            config.multicast_addr, config.multicast_port, config.max_clients
        );

        Self {
            config,
            root_dir,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            audit_enabled,
        }
    }

    /// Handle a multicast TFTP request
    ///
    /// RFC 2090: Process RRQ with multicast option
    ///
    /// NIST Controls:
    /// - AC-3: Access Enforcement (request validation)
    /// - SI-10: Information Input Validation (option parsing)
    /// - AU-2: Audit Events (request logging)
    pub async fn handle_multicast_request(
        &self,
        filename: String,
        mode: TransferMode,
        options: TftpOptions,
        client_addr: SocketAddr,
        response_socket: Arc<UdpSocket>,
    ) -> Result<()> {
        info!(
            "Multicast request from {}: file={}, mode={:?}",
            client_addr, filename, mode
        );

        // NIST AC-3: Validate and resolve file path
        let file_path = self.validate_file_path(&filename)?;

        // NIST SI-10: Validate file exists and is readable
        let _file_metadata = tokio::fs::metadata(&file_path)
            .await
            .map_err(|_| TftpError::Tftp("File not found".to_string()))?;

        // Find or create multicast session for this file
        let session_key = format!("{}:{:?}", filename, mode);
        let session = self
            .get_or_create_session(
                session_key.clone(),
                file_path.clone(),
                mode.clone(),
                options.clone(),
            )
            .await?;

        // Add client to session
        let mut session_lock = session.write().await;
        let is_master = session_lock.add_client(client_addr)?;

        // Audit log: Client joined multicast session
        if self.audit_enabled {
            AuditLogger::multicast_client_joined(
                session_lock.session_id(),
                client_addr,
                is_master,
                session_lock.client_count(),
            );
        }

        // RFC 2090: Send OACK with multicast option to client
        self.send_multicast_oack(&response_socket, client_addr, &session_lock, is_master)
            .await?;

        drop(session_lock);

        // If this is the first client (master), start the transfer
        if is_master {
            let session_clone = Arc::clone(&session);
            let config = self.config.clone();
            let audit_enabled = self.audit_enabled;
            tokio::spawn(async move {
                if let Err(e) =
                    Self::run_multicast_transfer(session_clone, config, audit_enabled).await
                {
                    error!("Multicast transfer failed: {}", e);
                }
            });
        }

        Ok(())
    }

    /// Get existing session or create new one
    ///
    /// NIST Controls:
    /// - AC-3: Access Enforcement (session management)
    /// - SC-5: Denial of Service Protection (session limits)
    async fn get_or_create_session(
        &self,
        session_key: String,
        file_path: PathBuf,
        mode: TransferMode,
        options: TftpOptions,
    ) -> Result<Arc<RwLock<MulticastSession>>> {
        let mut sessions = self.sessions.write().await;

        if let Some(existing) = sessions.get(&session_key) {
            return Ok(Arc::clone(existing));
        }

        // Create new session
        let session = Arc::new(RwLock::new(MulticastSession::new(
            file_path.clone(),
            mode,
            options,
            self.config.multicast_addr,
            self.config.multicast_port,
            self.config.max_clients,
        )));

        // Audit log: Multicast session created
        if self.audit_enabled {
            let session_lock = session.read().await;
            AuditLogger::multicast_session_created(
                session_lock.session_id(),
                &file_path.display().to_string(),
                &self.config.multicast_addr.to_string(),
                self.config.multicast_port,
            );
        }

        sessions.insert(session_key, Arc::clone(&session));
        Ok(session)
    }

    /// Send OACK with multicast parameters
    ///
    /// RFC 2090: Inform client of multicast group and role
    ///
    /// NIST Controls:
    /// - SC-8: Transmission Confidentiality and Integrity (protocol compliance)
    /// - AU-3: Content of Audit Records (log OACK details)
    async fn send_multicast_oack(
        &self,
        socket: &UdpSocket,
        client_addr: SocketAddr,
        session: &MulticastSession,
        is_master: bool,
    ) -> Result<()> {
        let mut packet = BytesMut::new();
        packet.put_u16(TftpOpcode::Oack as u16);

        // RFC 2090: multicast option format: "multicast,<addr>,<port>,<master>"
        let multicast_value = format!(
            "{},{},{}",
            session.multicast_addr,
            session.multicast_port,
            if is_master { "1" } else { "0" }
        );

        // Add multicast option
        packet.put_slice(MULTICAST_OPTION.as_bytes());
        packet.put_u8(0);
        packet.put_slice(multicast_value.as_bytes());
        packet.put_u8(0);

        // Add other negotiated options
        if session.options.block_size != DEFAULT_BLOCK_SIZE {
            packet.put_slice(b"blksize");
            packet.put_u8(0);
            packet.put_slice(session.options.block_size.to_string().as_bytes());
            packet.put_u8(0);
        }

        socket.send_to(&packet, client_addr).await?;
        info!(
            "Sent multicast OACK to {} (master: {}, group: {}:{})",
            client_addr, is_master, session.multicast_addr, session.multicast_port
        );

        Ok(())
    }

    /// Run the multicast file transfer
    ///
    /// RFC 2090: Multicast data transmission with ACK coordination
    ///
    /// NIST Controls:
    /// - SC-5: Denial of Service Protection (efficient transmission)
    /// - SC-5(2): Capacity, Bandwidth, and Redundancy (multicast optimization)
    /// - AU-2: Audit Events (transfer progress logging)
    async fn run_multicast_transfer(
        session: Arc<RwLock<MulticastSession>>,
        config: MulticastConfig,
        audit_enabled: bool,
    ) -> Result<()> {
        let (file_data, multicast_addr, multicast_port, block_size, mode) = {
            let session_lock = session.read().await;
            let mut file = File::open(&session_lock.file_path).await?;

            // Read and optionally convert file data
            let file_data = if session_lock.mode == TransferMode::Netascii {
                let mut raw_data = Vec::new();
                file.read_to_end(&mut raw_data).await?;
                TransferMode::convert_to_netascii(&raw_data)
            } else {
                let mut raw_data = Vec::new();
                file.read_to_end(&mut raw_data).await?;
                raw_data
            };

            (
                file_data,
                session_lock.multicast_addr,
                session_lock.multicast_port,
                session_lock.options.block_size,
                session_lock.mode.clone(),
            )
        };

        // Create multicast socket
        let socket = Self::create_multicast_socket(multicast_addr, multicast_port).await?;

        // Send file data in blocks
        let mut block_num: u16 = 1;
        let mut offset = 0;
        let retransmit_timeout = Duration::from_secs(config.retransmit_timeout_secs);

        while offset < file_data.len() {
            let bytes_to_send = std::cmp::min(block_size, file_data.len() - offset);
            let block_data = &file_data[offset..offset + bytes_to_send];

            // Send DATA packet to multicast group
            Self::send_multicast_data(
                &socket,
                block_num,
                block_data,
                multicast_addr,
                multicast_port,
            )
            .await?;

            // Wait for all clients to ACK (with timeout)
            let ack_result = timeout(retransmit_timeout, async {
                loop {
                    let session_lock = session.read().await;
                    if session_lock.all_clients_acked(block_num) {
                        return Ok::<(), TftpError>(());
                    }
                    drop(session_lock);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await;

            match ack_result {
                Ok(_) => {
                    // All clients ACKed, proceed to next block
                    debug!("Block {} acknowledged by all clients", block_num);
                }
                Err(_) => {
                    // Timeout: some clients didn't ACK
                    let mut session_lock = session.write().await;
                    let missing = session_lock.get_missing_clients(block_num);
                    warn!(
                        "Block {} timeout: {} clients missing ACK: {:?}",
                        block_num,
                        missing.len(),
                        missing
                    );
                    session_lock.queue_retransmit(block_num);
                }
            }

            offset += bytes_to_send;

            // RFC 1350: Transfer complete when data packet < block_size
            if bytes_to_send < block_size {
                info!(
                    "Multicast transfer complete: {} blocks sent ({} bytes, mode: {:?})",
                    block_num,
                    file_data.len(),
                    mode
                );
                break;
            }

            block_num = block_num.wrapping_add(1);
        }

        // Handle retransmissions
        Self::handle_retransmissions(
            session,
            &socket,
            &file_data,
            block_size,
            config,
            audit_enabled,
        )
        .await?;

        Ok(())
    }

    /// Create and configure multicast UDP socket
    ///
    /// RFC 2090: Set up multicast group transmission
    ///
    /// NIST Controls:
    /// - SC-7: Boundary Protection (multicast group isolation)
    /// - CM-6: Configuration Settings (socket configuration)
    async fn create_multicast_socket(
        multicast_addr: IpAddr,
        multicast_port: u16,
    ) -> Result<UdpSocket> {
        let bind_addr = match multicast_addr {
            IpAddr::V4(_) => "0.0.0.0:0",
            IpAddr::V6(_) => "[::]:0",
        };

        let socket = UdpSocket::bind(bind_addr).await?;

        // Enable multicast
        match multicast_addr {
            IpAddr::V4(addr) => {
                socket.set_multicast_ttl_v4(4)?; // Local network scope
                info!(
                    "Multicast socket created for IPv4: {}:{}",
                    addr, multicast_port
                );
            }
            IpAddr::V6(addr) => {
                socket.set_multicast_loop_v6(false)?;
                info!(
                    "Multicast socket created for IPv6: {}:{}",
                    addr, multicast_port
                );
            }
        }

        Ok(socket)
    }

    /// Send DATA packet to multicast group
    ///
    /// RFC 2090: Multicast data transmission
    ///
    /// NIST Controls:
    /// - SC-5: Denial of Service Protection (efficient multicast)
    async fn send_multicast_data(
        socket: &UdpSocket,
        block_num: u16,
        data: &[u8],
        multicast_addr: IpAddr,
        multicast_port: u16,
    ) -> Result<()> {
        let mut packet = BytesMut::with_capacity(4 + data.len());
        packet.put_u16(TftpOpcode::Data as u16);
        packet.put_u16(block_num);
        packet.put_slice(data);

        let dest = SocketAddr::new(multicast_addr, multicast_port);
        socket.send_to(&packet, dest).await?;

        debug!(
            "Sent multicast DATA block {} ({} bytes) to {}",
            block_num,
            data.len(),
            dest
        );

        Ok(())
    }

    /// Handle block retransmissions
    ///
    /// RFC 2090: Selective retransmission for missed blocks
    ///
    /// NIST Controls:
    /// - SC-5(2): Capacity, Bandwidth, and Redundancy (targeted retransmission)
    async fn handle_retransmissions(
        session: Arc<RwLock<MulticastSession>>,
        socket: &UdpSocket,
        file_data: &[u8],
        block_size: usize,
        config: MulticastConfig,
        audit_enabled: bool,
    ) -> Result<()> {
        let max_retries = 3;
        let retry_timeout = Duration::from_secs(config.retransmit_timeout_secs);

        for retry in 0..max_retries {
            let retransmit_blocks = {
                let mut session_lock = session.write().await;
                session_lock.remove_inactive_clients(config.master_timeout_secs * 2, audit_enabled);
                session_lock.take_retransmit_queue()
            };

            if retransmit_blocks.is_empty() {
                break;
            }

            info!(
                "Retransmission round {} for {} blocks: {:?}",
                retry + 1,
                retransmit_blocks.len(),
                retransmit_blocks
            );

            for block_num in retransmit_blocks {
                // Security: Use checked arithmetic to prevent integer overflow
                // Calculate offset safely: (block_num - 1) * block_size
                //
                // NIST 800-53 Controls:
                // - SI-10: Information Input Validation (validate arithmetic operations)
                // - SC-5: Denial of Service Protection (prevent integer overflow)
                //
                // STIG V-222577: Applications must validate all input
                // STIG V-222578: Applications must protect from integer overflow
                let block_index = (block_num - 1) as usize;
                let offset = match block_index.checked_mul(block_size) {
                    Some(off) => off,
                    None => {
                        error!("Block offset overflow for block {}", block_num);
                        continue;
                    }
                };

                if offset >= file_data.len() {
                    continue;
                }

                let bytes_to_send = std::cmp::min(block_size, file_data.len() - offset);
                let block_data = &file_data[offset..offset + bytes_to_send];

                let session_lock = session.read().await;
                Self::send_multicast_data(
                    socket,
                    block_num,
                    block_data,
                    session_lock.multicast_addr,
                    session_lock.multicast_port,
                )
                .await?;
                drop(session_lock);

                // Wait for ACKs
                tokio::time::sleep(retry_timeout).await;
            }
        }

        Ok(())
    }

    /// Validate and resolve file path for multicast transfers
    ///
    /// NIST 800-53 Controls:
    /// - AC-3: Access Enforcement (path validation)
    /// - SI-10: Information Input Validation (prevent traversal)
    /// - SC-7(12): Host-Based Boundary Protection (filesystem boundary enforcement)
    /// - AC-6: Least Privilege (restrict to authorized directories)
    ///
    /// STIG V-222602: Applications must enforce access restrictions
    /// STIG V-222603: Applications must protect against directory traversal
    /// STIG V-222604: Applications must validate file paths
    /// STIG V-222612: Applications must implement path canonicalization
    fn validate_file_path(&self, filename: &str) -> Result<PathBuf> {
        // NIST SI-10: Normalize and validate filename
        // STIG V-222603: Prevent path traversal
        let filename = filename.replace('\\', "/");
        if filename.contains("..") {
            return Err(TftpError::Tftp("Invalid filename".to_string()));
        }

        // NIST AC-3: Enforce base path restriction
        let file_path = self.root_dir.join(filename.trim_start_matches('/'));

        // Security: Detect and reject symlinks to prevent TOCTOU attacks
        // NIST AC-3: Additional access control validation
        // STIG V-222604: Validate file type
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

        // NIST SC-7(12): Enforce filesystem boundary
        // STIG V-222612: Path canonicalization
        let canonical_root = self
            .root_dir
            .canonicalize()
            .map_err(|_| TftpError::Tftp("Root directory error".to_string()))?;

        // NIST AC-6: Least privilege boundary check
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
}
