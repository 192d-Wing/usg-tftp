// Phase 4: Worker Thread Pool
//
// NGINX-style multi-threaded architecture for multi-core CPU utilization.
//
// Architecture:
// - Master thread: Dedicated to batch receiving packets via recvmmsg()
// - Worker threads: Process TFTP client requests in parallel
// - Sender thread: Batch sending responses via sendmmsg()
//
// Expected improvements:
// - 2-4x concurrent client capacity
// - 4-8x CPU core utilization
// - 30-50% latency reduction under load

use crate::config::{LoadBalanceStrategy, TftpConfig, WriteConfig};
use crate::error::{Result, TftpError};
use bytes::BytesMut;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use nix::sys::socket::{MsgFlags, MultiHeaders, SockaddrStorage, recvmmsg, sendmmsg};
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use std::os::unix::io::AsRawFd;

const MAX_PACKET_SIZE: usize = 65468;

/// Incoming packet from master to worker
#[derive(Debug)]
pub struct IncomingPacket {
    /// Raw packet data
    pub data: Vec<u8>,
    /// Client socket address
    pub addr: SocketAddr,
    /// Reception timestamp for metrics
    pub timestamp: Instant,
}

/// Outgoing response from worker to sender
#[derive(Debug)]
pub struct OutgoingPacket {
    /// Response packet data
    pub data: Vec<u8>,
    /// Destination address
    pub addr: SocketAddr,
    /// Original timestamp for latency tracking
    pub timestamp: Instant,
}

/// Master thread statistics
#[derive(Debug, Default)]
pub struct MasterStats {
    pub packets_received: AtomicU64,
    pub packets_dropped: AtomicU64,
    pub batches_received: AtomicU64,
    pub errors: AtomicU64,
}

/// Worker thread statistics
#[derive(Debug)]
pub struct WorkerStats {
    pub worker_id: usize,
    pub packets_processed: AtomicU64,
    pub total_processing_time_us: AtomicU64,
    pub errors: AtomicU64,
}

impl WorkerStats {
    pub fn new(worker_id: usize) -> Self {
        Self {
            worker_id,
            packets_processed: AtomicU64::new(0),
            total_processing_time_us: AtomicU64::new(0),
            errors: AtomicU64::new(0),
        }
    }
}

/// Sender thread statistics
#[derive(Debug, Default)]
pub struct SenderStats {
    pub packets_sent: AtomicU64,
    pub batches_sent: AtomicU64,
    pub errors: AtomicU64,
}

/// Worker thread pool handle
pub struct WorkerPool {
    /// Worker channels for sending packets to workers
    worker_senders: Vec<mpsc::Sender<IncomingPacket>>,
    /// Worker receivers for processing packets
    worker_receivers: Vec<mpsc::Receiver<IncomingPacket>>,
    /// Sender channel for workers to send responses
    sender_tx: mpsc::Sender<OutgoingPacket>,
    /// Sender channel for receiving responses from workers
    sender_receiver: mpsc::Receiver<OutgoingPacket>,
    /// Configuration
    config: Arc<TftpConfig>,
    /// Statistics
    master_stats: Arc<MasterStats>,
    worker_stats: Vec<Arc<WorkerStats>>,
    sender_stats: Arc<SenderStats>,
}

impl WorkerPool {
    /// Create a new worker pool
    pub fn new(config: Arc<TftpConfig>) -> Self {
        let worker_count = config.performance.platform.worker_pool.worker_count;
        let worker_channel_size = config.performance.platform.worker_pool.worker_channel_size;
        let sender_channel_size = config.performance.platform.worker_pool.sender_channel_size;

        // Create channels between master and workers
        let mut worker_senders = Vec::with_capacity(worker_count);
        let mut worker_receivers = Vec::with_capacity(worker_count);
        let worker_stats: Vec<Arc<WorkerStats>> = (0..worker_count)
            .map(|id| Arc::new(WorkerStats::new(id)))
            .collect();

        // Create channel between workers and sender
        let (sender_tx, sender_rx) = mpsc::channel::<OutgoingPacket>(sender_channel_size);

        info!(
            "Creating worker pool with {} workers, channel sizes: worker={}, sender={}",
            worker_count, worker_channel_size, sender_channel_size
        );

        // Create worker channels
        for _worker_id in 0..worker_count {
            let (tx, rx) = mpsc::channel::<IncomingPacket>(worker_channel_size);
            worker_senders.push(tx);
            worker_receivers.push(rx);
        }

        Self {
            worker_senders,
            worker_receivers,
            sender_tx,
            sender_receiver: sender_rx,
            config,
            master_stats: Arc::new(MasterStats::default()),
            worker_stats,
            sender_stats: Arc::new(SenderStats::default()),
        }
    }

    /// Start the worker pool
    ///
    /// Spawns:
    /// - Master receiver thread
    /// - N worker threads
    /// - Sender thread
    pub async fn start(
        self,
        socket: Arc<UdpSocket>,
        _root_dir: std::path::PathBuf,
        _write_config: WriteConfig,
        _max_file_size_bytes: u64,
        _audit_enabled: bool,
        _multicast_server: Option<Arc<crate::multicast::MulticastTftpServer>>,
    ) -> Result<()> {
        let worker_count = self.config.performance.platform.worker_pool.worker_count;

        info!("Starting worker pool with {} workers", worker_count);

        // Clone stats references before moving self
        let master_stats = self.master_stats.clone();
        let worker_stats = self.worker_stats.clone();
        let sender_stats = self.sender_stats.clone();

        // Spawn master receiver thread
        let _master_handle = {
            let socket = socket.clone();
            let config = self.config.clone();
            let workers = self.worker_senders.clone();
            let stats = self.master_stats.clone();

            tokio::spawn(async move {
                if let Err(e) = master_receiver_loop(socket, workers, config, stats).await {
                    error!("Master receiver loop failed: {}", e);
                }
            })
        };

        // Spawn worker threads
        let mut _worker_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();
        for (worker_id, rx) in self.worker_receivers.into_iter().enumerate() {
            let sender_tx = self.sender_tx.clone();
            let config = self.config.clone();
            let stats = self.worker_stats[worker_id].clone();
            let root_dir = _root_dir.clone();
            let write_config = _write_config.clone();
            let max_file_size = _max_file_size_bytes;
            let audit_enabled = _audit_enabled;
            let multicast_server = _multicast_server.clone();

            let handle = tokio::spawn(async move {
                if let Err(e) = worker_thread(
                    worker_id,
                    rx,
                    sender_tx,
                    config,
                    stats,
                    root_dir,
                    write_config,
                    max_file_size,
                    audit_enabled,
                    multicast_server,
                )
                .await
                {
                    error!("Worker {} failed: {}", worker_id, e);
                }
            });

            _worker_handles.push(handle);
        }

        // Spawn sender thread
        let _sender_handle = {
            let socket = socket.clone();
            let config = self.config.clone();
            let rx = self.sender_receiver;
            let stats = self.sender_stats.clone();

            tokio::spawn(async move {
                if let Err(e) = sender_thread(rx, socket, config, stats).await {
                    error!("Sender thread failed: {}", e);
                }
            })
        };

        info!("Worker pool started successfully");

        // Keep pool alive
        tokio::signal::ctrl_c().await?;
        info!("Shutdown signal received, stopping worker pool");

        // Print final statistics
        print_stats_impl(&master_stats, &worker_stats, &sender_stats);

        Ok(())
    }

    /// Get master statistics
    pub fn master_stats(&self) -> &MasterStats {
        &self.master_stats
    }

    /// Get worker statistics
    pub fn worker_stats(&self, worker_id: usize) -> Option<&WorkerStats> {
        self.worker_stats.get(worker_id).map(|s| s.as_ref())
    }

    /// Get sender statistics
    pub fn sender_stats(&self) -> &SenderStats {
        &self.sender_stats
    }

    /// Print statistics
    pub fn print_stats(&self) {
        info!("=== Worker Pool Statistics ===");

        info!(
            "Master: received={}, batches={}, dropped={}, errors={}",
            self.master_stats.packets_received.load(Ordering::Relaxed),
            self.master_stats.batches_received.load(Ordering::Relaxed),
            self.master_stats.packets_dropped.load(Ordering::Relaxed),
            self.master_stats.errors.load(Ordering::Relaxed),
        );

        for stats in &self.worker_stats {
            let processed = stats.packets_processed.load(Ordering::Relaxed);
            let total_time = stats.total_processing_time_us.load(Ordering::Relaxed);
            let avg_time = if processed > 0 {
                total_time / processed
            } else {
                0
            };

            info!(
                "Worker {}: processed={}, avg_time={}us, errors={}",
                stats.worker_id,
                processed,
                avg_time,
                stats.errors.load(Ordering::Relaxed),
            );
        }

        info!(
            "Sender: sent={}, batches={}, errors={}",
            self.sender_stats.packets_sent.load(Ordering::Relaxed),
            self.sender_stats.batches_sent.load(Ordering::Relaxed),
            self.sender_stats.errors.load(Ordering::Relaxed),
        );
    }
}

/// Print statistics (standalone function)
fn print_stats_impl(
    master_stats: &Arc<MasterStats>,
    worker_stats: &[Arc<WorkerStats>],
    sender_stats: &Arc<SenderStats>,
) {
    info!("=== Worker Pool Statistics ===");

    info!(
        "Master: received={}, batches={}, dropped={}, errors={}",
        master_stats.packets_received.load(Ordering::Relaxed),
        master_stats.batches_received.load(Ordering::Relaxed),
        master_stats.packets_dropped.load(Ordering::Relaxed),
        master_stats.errors.load(Ordering::Relaxed),
    );

    for stats in worker_stats {
        let processed = stats.packets_processed.load(Ordering::Relaxed);
        let total_time = stats.total_processing_time_us.load(Ordering::Relaxed);
        let avg_time = if processed > 0 {
            total_time / processed
        } else {
            0
        };

        info!(
            "Worker {}: processed={}, avg_time={}us, errors={}",
            stats.worker_id,
            processed,
            avg_time,
            stats.errors.load(Ordering::Relaxed),
        );
    }

    info!(
        "Sender: sent={}, batches={}, errors={}",
        sender_stats.packets_sent.load(Ordering::Relaxed),
        sender_stats.batches_sent.load(Ordering::Relaxed),
        sender_stats.errors.load(Ordering::Relaxed),
    );
}

/// Select worker based on load balancing strategy
pub fn select_worker(
    strategy: LoadBalanceStrategy,
    client_addr: &SocketAddr,
    worker_count: usize,
    round_robin_counter: &mut usize,
) -> usize {
    match strategy {
        LoadBalanceStrategy::RoundRobin => {
            let idx = *round_robin_counter % worker_count;
            *round_robin_counter = round_robin_counter.wrapping_add(1);
            idx
        }
        LoadBalanceStrategy::ClientHash => {
            // Hash client IP:port for session affinity
            let mut hasher = DefaultHasher::new();
            client_addr.hash(&mut hasher);
            hasher.finish() as usize % worker_count
        }
        LoadBalanceStrategy::LeastLoaded => {
            // TODO: Implement least-loaded strategy
            // For now, fall back to round-robin
            let idx = *round_robin_counter % worker_count;
            *round_robin_counter = round_robin_counter.wrapping_add(1);
            idx
        }
    }
}

/// Master receiver loop: Batch receive packets and distribute to workers
async fn master_receiver_loop(
    socket: Arc<UdpSocket>,
    workers: Vec<mpsc::Sender<IncomingPacket>>,
    config: Arc<TftpConfig>,
    stats: Arc<MasterStats>,
) -> Result<()> {
    let batch_size = config.performance.platform.batch.max_batch_size;
    let batch_timeout_us = config.performance.platform.batch.batch_timeout_us;
    let strategy = config
        .performance
        .platform
        .worker_pool
        .load_balance_strategy;
    let worker_count = workers.len();

    let mut worker_index: usize = 0;

    info!(
        "Master receiver starting: batch_size={}, timeout={}μs, strategy={:?}, workers={}",
        batch_size, batch_timeout_us, strategy, worker_count
    );

    loop {
        // Batch receive packets
        let packets = match batch_recv_packets_internal(&socket, batch_size, batch_timeout_us).await
        {
            Ok(pkts) if !pkts.is_empty() => pkts,
            Ok(_) => {
                // Timeout with no packets, continue
                continue;
            }
            Err(e) => {
                error!("Master receiver error: {}", e);
                stats.errors.fetch_add(1, Ordering::Relaxed);
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                continue;
            }
        };

        let timestamp = Instant::now();
        stats
            .packets_received
            .fetch_add(packets.len() as u64, Ordering::Relaxed);
        stats.batches_received.fetch_add(1, Ordering::Relaxed);

        // Distribute packets to workers
        for (data, client_addr) in packets {
            let packet = IncomingPacket {
                data,
                addr: client_addr,
                timestamp,
            };

            // Select worker based on strategy
            let worker_idx = select_worker(strategy, &client_addr, worker_count, &mut worker_index);

            // Send to worker (non-blocking)
            if let Err(e) = workers[worker_idx].try_send(packet) {
                warn!("Worker {} channel full, dropping packet: {}", worker_idx, e);
                stats.packets_dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// Internal batch receive function
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
async fn batch_recv_packets_internal(
    socket: &UdpSocket,
    batch_size: usize,
    timeout_us: u64,
) -> Result<Vec<(Vec<u8>, SocketAddr)>> {
    use nix::sys::time::TimeSpec;
    use std::io::IoSliceMut;
    use std::time::Duration;

    let socket_fd = socket.as_raw_fd();

    // Prepare buffers
    let mut buffers: Vec<Vec<u8>> = (0..batch_size)
        .map(|_| vec![0u8; MAX_PACKET_SIZE])
        .collect();

    let mut iovecs: Vec<Vec<IoSliceMut>> = buffers
        .iter_mut()
        .map(|buf| vec![IoSliceMut::new(buf)])
        .collect();

    let mut headers = MultiHeaders::<SockaddrStorage>::preallocate(batch_size, None);

    let timeout = if timeout_us > 0 {
        Some(TimeSpec::from_duration(Duration::from_micros(timeout_us)))
    } else {
        None
    };

    // Perform batch receive
    match recvmmsg(
        socket_fd,
        &mut headers,
        iovecs.iter_mut(),
        MsgFlags::empty(),
        timeout,
    ) {
        Ok(msgs_received) => {
            let mut results = Vec::new();

            for (i, msg) in msgs_received.into_iter().enumerate() {
                if let Some(addr_storage) = msg.address {
                    let addr = sockaddr_to_std(&addr_storage)?;
                    let data = buffers[i][..msg.bytes].to_vec();
                    results.push((data, addr));
                }
            }

            Ok(results)
        }
        Err(nix::errno::Errno::EAGAIN) | Err(nix::errno::Errno::EWOULDBLOCK) => Ok(Vec::new()),
        Err(e) => Err(TftpError::Tftp(format!("recvmmsg error: {}", e))),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
async fn batch_recv_packets_internal(
    _socket: &UdpSocket,
    _batch_size: usize,
    _timeout_us: u64,
) -> Result<Vec<(Vec<u8>, SocketAddr)>> {
    Err(TftpError::Tftp(
        "recvmmsg not supported on this platform".into(),
    ))
}

/// Sender thread: Batch send outgoing packets
async fn sender_thread(
    mut rx: mpsc::Receiver<OutgoingPacket>,
    socket: Arc<UdpSocket>,
    config: Arc<TftpConfig>,
    stats: Arc<SenderStats>,
) -> Result<()> {
    let batch_size = config.performance.platform.batch.max_batch_size;
    let batch_timeout =
        tokio::time::Duration::from_micros(config.performance.platform.batch.batch_timeout_us);

    let mut batch = Vec::with_capacity(batch_size);

    info!(
        "Sender thread starting: batch_size={}, timeout={:?}",
        batch_size, batch_timeout
    );

    loop {
        // Collect responses for batching
        tokio::select! {
            opt = rx.recv() => {
                if let Some(packet) = opt {
                    batch.push(packet);

                    // Fill rest of batch non-blocking
                    while batch.len() < batch_size {
                        match rx.try_recv() {
                            Ok(pkt) => batch.push(pkt),
                            Err(_) => break,
                        }
                    }
                } else {
                    // Channel closed, shutdown
                    info!("Sender channel closed, shutting down");
                    break;
                }
            }
            _ = tokio::time::sleep(batch_timeout), if !batch.is_empty() => {
                // Timeout expired, send what we have
            }
        }

        // Send batch
        if !batch.is_empty() {
            match batch_send_packets_internal(&socket, &batch).await {
                Ok(sent) => {
                    stats.packets_sent.fetch_add(sent as u64, Ordering::Relaxed);
                    stats.batches_sent.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    error!("Sender thread error: {}", e);
                    stats.errors.fetch_add(1, Ordering::Relaxed);
                }
            }
            batch.clear();
        }
    }

    info!("Sender thread shutting down");
    Ok(())
}

/// Worker thread: Process TFTP packets from master and send responses to sender
async fn worker_thread(
    worker_id: usize,
    mut rx: mpsc::Receiver<IncomingPacket>,
    tx: mpsc::Sender<OutgoingPacket>,
    config: Arc<TftpConfig>,
    stats: Arc<WorkerStats>,
    root_dir: std::path::PathBuf,
    write_config: WriteConfig,
    max_file_size: u64,
    audit_enabled: bool,
    multicast_server: Option<Arc<crate::multicast::MulticastTftpServer>>,
) -> Result<()> {
    info!("Worker {} starting", worker_id);

    while let Some(packet) = rx.recv().await {
        let start = std::time::Instant::now();

        // Process TFTP packet
        match process_tftp_packet(
            packet,
            &tx,
            &config,
            &root_dir,
            &write_config,
            max_file_size,
            audit_enabled,
            multicast_server.as_ref(),
        )
        .await
        {
            Ok(_) => {
                // Packet processed successfully
            }
            Err(e) => {
                error!("Worker {}: Error processing packet: {}", worker_id, e);
                stats.errors.fetch_add(1, Ordering::Relaxed);
            }
        }

        // Update statistics
        let elapsed = start.elapsed().as_micros() as u64;
        stats.packets_processed.fetch_add(1, Ordering::Relaxed);
        stats
            .total_processing_time_us
            .fetch_add(elapsed, Ordering::Relaxed);
    }

    info!("Worker {} shutting down", worker_id);
    Ok(())
}

/// Process a single TFTP packet and generate responses
///
/// This function handles initial TFTP requests (RRQ/WRQ). For ongoing transfers
/// (DATA/ACK packets), those are handled by existing per-client tasks spawned
/// by handle_read_request and handle_write_request.
///
/// The worker pool architecture is most beneficial for the initial packet processing
/// and distributing load across cores. Once a transfer is established, the existing
/// single-threaded async architecture is efficient for that session.
async fn process_tftp_packet(
    packet: IncomingPacket,
    tx: &mpsc::Sender<OutgoingPacket>,
    config: &TftpConfig,
    root_dir: &std::path::Path,
    write_config: &WriteConfig,
    max_file_size: u64,
    audit_enabled: bool,
    multicast_server: Option<&Arc<crate::multicast::MulticastTftpServer>>,
) -> Result<()> {
    use bytes::{Buf, BytesMut};

    let mut bytes = BytesMut::from(&packet.data[..]);

    // Validate minimum packet size
    if bytes.len() < 2 {
        send_error_response(tx, packet.addr, packet.timestamp, 4, "Packet too small").await?;
        return Ok(());
    }

    let opcode = bytes.get_u16();

    // Parse opcode
    let opcode = match opcode {
        1 => TftpOpcode::Rrq,
        2 => TftpOpcode::Wrq,
        3 => TftpOpcode::Data,
        4 => TftpOpcode::Ack,
        5 => TftpOpcode::Error,
        6 => TftpOpcode::Oack,
        _ => {
            send_error_response(
                tx,
                packet.addr,
                packet.timestamp,
                4,
                "Illegal TFTP operation",
            )
            .await?;
            return Ok(());
        }
    };

    match opcode {
        TftpOpcode::Rrq => {
            // Handle Read Request
            if let Err(e) = handle_read_request_worker(
                packet.addr,
                &mut bytes,
                root_dir,
                config,
                max_file_size,
                audit_enabled,
                multicast_server,
            )
            .await
            {
                error!("Read request failed: {}", e);
                send_error_response(tx, packet.addr, packet.timestamp, 0, &e.to_string()).await?;
            }
        }
        TftpOpcode::Wrq => {
            // Handle Write Request
            if let Err(e) = handle_write_request_worker(
                packet.addr,
                &mut bytes,
                root_dir,
                write_config,
                config,
                max_file_size,
                audit_enabled,
            )
            .await
            {
                error!("Write request failed: {}", e);
                send_error_response(tx, packet.addr, packet.timestamp, 0, &e.to_string()).await?;
            }
        }
        TftpOpcode::Data | TftpOpcode::Ack => {
            // These packets are part of ongoing transfers
            // In the current architecture, these should not reach workers because
            // ongoing transfers create connected sockets that receive directly
            debug!(
                "Worker received {:?} packet from {} - ongoing transfers should use connected sockets",
                opcode, packet.addr
            );
        }
        TftpOpcode::Error => {
            debug!("Worker received ERROR packet from {}", packet.addr);
        }
        TftpOpcode::Oack => {
            warn!(
                "Worker received OACK packet from {} - unexpected (OACK is server->client)",
                packet.addr
            );
        }
    }

    Ok(())
}

/// Handle Read Request in worker thread
async fn handle_read_request_worker(
    client_addr: std::net::SocketAddr,
    bytes: &mut BytesMut,
    root_dir: &std::path::Path,
    config: &TftpConfig,
    max_file_size: u64,
    audit_enabled: bool,
    _multicast_server: Option<&Arc<crate::multicast::MulticastTftpServer>>,
) -> Result<()> {
    use crate::{TftpOptions, TransferMode};

    // Parse request packet
    let (filename, mode_str, requested_options) = parse_request_packet(bytes)?;

    // Validate and parse transfer mode
    let mode = TransferMode::from_str(&mode_str)?;
    if mode == TransferMode::Mail {
        return Err(TftpError::Tftp("MAIL mode not supported".into()));
    }

    info!(
        "Worker processing RRQ: file={}, mode={:?}, from={}",
        filename, mode, client_addr
    );

    // Validate and resolve file path (prevent directory traversal)
    let file_path = crate::TftpServer::validate_and_resolve_path(root_dir, &filename)?;

    // Negotiate options
    let default_windowsize = config.performance.platform.batch.max_batch_size;
    let mut options = TftpOptions {
        windowsize: default_windowsize,
        ..TftpOptions::default()
    };
    let mut negotiated_options = std::collections::HashMap::new();

    for (name, value) in &requested_options {
        match name.as_str() {
            "blksize" => {
                if let Ok(size) = value.parse::<usize>() {
                    if (8..=crate::MAX_BLOCK_SIZE).contains(&size) {
                        options.block_size = size;
                        negotiated_options.insert("blksize".to_string(), size.to_string());
                    }
                }
            }
            "timeout" => {
                if let Ok(t) = value.parse::<u64>() {
                    if (1..=255).contains(&t) {
                        options.timeout = t;
                        negotiated_options.insert("timeout".to_string(), t.to_string());
                    }
                }
            }
            "tsize" => {
                // For RRQ, server responds with actual file size
                negotiated_options.insert("tsize".to_string(), "0".to_string());
            }
            "windowsize" => {
                if let Ok(w) = value.parse::<usize>() {
                    if (1..=65535).contains(&w) {
                        options.windowsize = w;
                        negotiated_options.insert("windowsize".to_string(), w.to_string());
                    }
                }
            }
            _ => {
                // Unknown option - ignore per RFC 2347
            }
        }
    }

    // Clone config for spawned task (required for 'static lifetime)
    let file_io_config = config.performance.platform.file_io.clone();

    // Spawn transfer task using existing handle_read_request
    tokio::spawn(async move {
        if let Err(e) = crate::TftpServer::handle_read_request(
            file_path,
            client_addr,
            mode,
            options,
            negotiated_options,
            max_file_size,
            audit_enabled,
            &file_io_config,
        )
        .await
        {
            error!("Read transfer failed for {}: {}", client_addr, e);
        }
    });

    Ok(())
}

/// Handle Write Request in worker thread
async fn handle_write_request_worker(
    client_addr: std::net::SocketAddr,
    bytes: &mut BytesMut,
    root_dir: &std::path::Path,
    write_config: &WriteConfig,
    config: &TftpConfig,
    max_file_size: u64,
    audit_enabled: bool,
) -> Result<()> {
    use crate::{TftpOptions, TransferMode};

    // Parse request packet
    let (filename, mode_str, requested_options) = parse_request_packet(bytes)?;

    // Validate and parse transfer mode
    let mode = TransferMode::from_str(&mode_str)?;
    if mode == TransferMode::Mail {
        return Err(TftpError::Tftp("MAIL mode not supported".into()));
    }

    info!(
        "Worker processing WRQ: file={}, mode={:?}, from={}",
        filename, mode, client_addr
    );

    // Check if writes are enabled
    if !write_config.enabled {
        return Err(TftpError::Tftp("Write operations not allowed".into()));
    }

    // Validate and resolve file path
    let file_path = crate::TftpServer::validate_and_resolve_path(root_dir, &filename)?;

    // Check if file exists
    let file_exists = tokio::fs::metadata(&file_path).await.is_ok();
    let file_created = !file_exists;

    if file_exists && !write_config.allow_overwrite {
        return Err(TftpError::Tftp("File already exists".into()));
    }

    // Negotiate options
    let default_windowsize = config.performance.platform.batch.max_batch_size;
    let mut options = TftpOptions {
        windowsize: default_windowsize,
        ..TftpOptions::default()
    };
    let mut negotiated_options = std::collections::HashMap::new();

    for (name, value) in &requested_options {
        match name.as_str() {
            "blksize" => {
                if let Ok(size) = value.parse::<usize>() {
                    if (8..=crate::MAX_BLOCK_SIZE).contains(&size) {
                        options.block_size = size;
                        negotiated_options.insert("blksize".to_string(), size.to_string());
                    }
                }
            }
            "timeout" => {
                if let Ok(t) = value.parse::<u64>() {
                    if (1..=255).contains(&t) {
                        options.timeout = t;
                        negotiated_options.insert("timeout".to_string(), t.to_string());
                    }
                }
            }
            "tsize" => {
                // For WRQ, acknowledge the transfer size
                negotiated_options.insert("tsize".to_string(), value.clone());
            }
            "windowsize" => {
                if let Ok(w) = value.parse::<usize>() {
                    if (1..=65535).contains(&w) {
                        options.windowsize = w;
                        negotiated_options.insert("windowsize".to_string(), w.to_string());
                    }
                }
            }
            _ => {
                // Unknown option - ignore per RFC 2347
            }
        }
    }

    // Spawn transfer task using existing handle_write_request
    tokio::spawn(async move {
        if let Err(e) = crate::TftpServer::handle_write_request(
            file_path,
            client_addr,
            mode,
            options,
            negotiated_options,
            max_file_size,
            file_created,
            audit_enabled,
        )
        .await
        {
            error!("Write transfer failed for {}: {}", client_addr, e);
        }
    });

    Ok(())
}

/// Parse a TFTP request packet (RRQ or WRQ)
fn parse_request_packet(
    bytes: &mut BytesMut,
) -> Result<(String, String, std::collections::HashMap<String, String>)> {
    use bytes::Buf;

    // Parse filename (null-terminated string)
    let filename = parse_null_terminated_string(bytes)?;

    // Parse mode (null-terminated string)
    let mode = parse_null_terminated_string(bytes)?;

    // Parse options (pairs of null-terminated strings)
    let mut options = std::collections::HashMap::new();
    while bytes.remaining() > 0 {
        match parse_null_terminated_string(bytes) {
            Ok(option_name) => match parse_null_terminated_string(bytes) {
                Ok(option_value) => {
                    options.insert(option_name.to_lowercase(), option_value);
                }
                Err(_) => break,
            },
            Err(_) => break,
        }
    }

    Ok((filename, mode, options))
}

/// Parse a null-terminated string from a byte buffer
fn parse_null_terminated_string(bytes: &mut BytesMut) -> Result<String> {
    use bytes::Buf;

    let mut result = Vec::new();
    while bytes.remaining() > 0 {
        let byte = bytes.get_u8();
        if byte == 0 {
            return String::from_utf8(result)
                .map_err(|e| TftpError::Tftp(format!("Invalid UTF-8 in string: {}", e)));
        }
        result.push(byte);
    }

    Err(TftpError::Tftp("Missing null terminator".to_string()))
}

/// Send an error response via the sender channel
async fn send_error_response(
    tx: &mpsc::Sender<OutgoingPacket>,
    addr: SocketAddr,
    timestamp: Instant,
    error_code: u16,
    error_msg: &str,
) -> Result<()> {
    use bytes::BufMut;

    // Build error packet: opcode (2 bytes) + error code (2 bytes) + message (null-terminated)
    let mut data = BytesMut::with_capacity(4 + error_msg.len() + 1);
    data.put_u16(5); // ERROR opcode
    data.put_u16(error_code);
    data.put(error_msg.as_bytes());
    data.put_u8(0); // null terminator

    let response = OutgoingPacket {
        data: data.to_vec(),
        addr,
        timestamp,
    };

    tx.send(response)
        .await
        .map_err(|e| TftpError::Tftp(format!("Failed to send error response: {}", e)))?;

    Ok(())
}

/// TFTP opcode enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TftpOpcode {
    Rrq = 1,
    Wrq = 2,
    Data = 3,
    Ack = 4,
    Error = 5,
    Oack = 6,
}

/// Internal batch send function
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
async fn batch_send_packets_internal(
    socket: &UdpSocket,
    packets: &[OutgoingPacket],
) -> Result<usize> {
    use nix::sys::socket::ControlMessage;
    use std::io::IoSlice;

    let socket_fd = socket.as_raw_fd();

    let mut iovecs: Vec<Vec<IoSlice>> = Vec::with_capacity(packets.len());
    let mut addrs: Vec<Option<SockaddrStorage>> = Vec::with_capacity(packets.len());

    for packet in packets {
        iovecs.push(vec![IoSlice::new(&packet.data)]);
        addrs.push(Some(SockaddrStorage::from(packet.addr)));
    }

    let control_msgs: Vec<Vec<ControlMessage>> = vec![vec![]; packets.len()];

    match sendmmsg(
        socket_fd,
        &mut MultiHeaders::preallocate(packets.len(), None),
        &iovecs,
        &control_msgs,
        &addrs,
        MsgFlags::empty(),
    ) {
        Ok(sent) => Ok(sent),
        Err(e) => Err(TftpError::Tftp(format!("sendmmsg error: {}", e))),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
async fn batch_send_packets_internal(
    _socket: &UdpSocket,
    _packets: &[OutgoingPacket],
) -> Result<usize> {
    Err(TftpError::Tftp(
        "sendmmsg not supported on this platform".into(),
    ))
}

/// Helper: Convert SockaddrStorage to std::net::SocketAddr
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn sockaddr_to_std(addr_storage: &SockaddrStorage) -> Result<SocketAddr> {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    if let Some(sock_addr) = addr_storage.as_sockaddr_in() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::from(sock_addr.ip())), sock_addr.port());
        Ok(addr)
    } else if let Some(sock_addr) = addr_storage.as_sockaddr_in6() {
        let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::from(sock_addr.ip())), sock_addr.port());
        Ok(addr)
    } else {
        Err(TftpError::Tftp("Unsupported address family".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn test_select_worker_round_robin() {
        let mut counter = 0;
        let worker_count = 4;
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345);

        for i in 0..8 {
            let worker_id = select_worker(
                LoadBalanceStrategy::RoundRobin,
                &addr,
                worker_count,
                &mut counter,
            );
            assert_eq!(worker_id, i % worker_count);
        }
    }

    #[test]
    fn test_select_worker_client_hash() {
        let mut counter = 0;
        let worker_count = 4;

        let addr1 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 12345);
        let addr2 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 101)), 12345);

        // Same client should always map to same worker
        let worker1 = select_worker(
            LoadBalanceStrategy::ClientHash,
            &addr1,
            worker_count,
            &mut counter,
        );
        let worker1_again = select_worker(
            LoadBalanceStrategy::ClientHash,
            &addr1,
            worker_count,
            &mut counter,
        );
        assert_eq!(worker1, worker1_again);

        // Different clients may map to different workers
        let worker2 = select_worker(
            LoadBalanceStrategy::ClientHash,
            &addr2,
            worker_count,
            &mut counter,
        );
        // Note: They might happen to hash to the same worker, so we just check validity
        assert!(worker2 < worker_count);
    }
}
