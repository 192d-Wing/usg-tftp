// Snow-Owl TFTP Client Binary
#![allow(dead_code)]

use snow_owl_tftp::{
    Result, TftpError, Opcode, TransferMode,
    DEFAULT_BLOCK_SIZE, MAX_BLOCK_SIZE, MAX_RETRIES,
};

use bytes::{Buf, BufMut, BytesMut};
use clap::Parser;
use socket2::{Socket, Domain, Type, Protocol};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;
use tokio::time::timeout;
use tracing::{debug, info, warn};

/// Snow-Owl TFTP Client
#[derive(Parser, Debug)]
#[command(name = "snow-owl-tftp-client")]
#[command(about = "High-performance TFTP client", long_about = None)]
struct Cli {
    /// TFTP server address (e.g., 192.168.1.100:69)
    #[arg(short, long)]
    server: String,

    /// Get file from server
    #[arg(short, long, conflicts_with = "put")]
    get: Option<String>,

    /// Put file to server
    #[arg(short, long, conflicts_with = "get")]
    put: Option<String>,

    /// Local file path (for get: destination, for put: source)
    #[arg(short, long)]
    file: Option<PathBuf>,

    /// Transfer mode (octet or netascii)
    #[arg(short, long, default_value = "octet")]
    mode: String,

    /// Block size (512-65464 bytes)
    #[arg(short, long, default_value_t = 8192)]
    block_size: usize,

    /// Timeout in seconds
    #[arg(short, long, default_value_t = 5)]
    timeout: u64,

    /// Window size (RFC 7440)
    #[arg(short, long, default_value_t = 16)]
    windowsize: usize,

    /// Verbose output
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    let log_level = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(log_level)
        .with_target(false)
        .init();

    // Parse server address
    let server_addr: SocketAddr = cli.server.parse()
        .map_err(|e| TftpError::Tftp(format!("Invalid server address: {}", e)))?;

    // Parse transfer mode
    let mode = TransferMode::from_str(&cli.mode)?;

    // Validate block size
    let block_size = if cli.block_size < 8 || cli.block_size > MAX_BLOCK_SIZE {
        warn!("Invalid block size {}, using default {}", cli.block_size, DEFAULT_BLOCK_SIZE);
        DEFAULT_BLOCK_SIZE
    } else {
        cli.block_size
    };

    // Create TFTP client
    let mut client = TftpClient::new(server_addr, mode, block_size, cli.timeout, cli.windowsize)?;

    // Execute operation
    if let Some(remote_file) = cli.get {
        let local_file = cli.file.unwrap_or_else(|| PathBuf::from(&remote_file));
        info!("Downloading {} from {} to {:?}", remote_file, server_addr, local_file);
        client.get(&remote_file, &local_file).await?;
        info!("Download complete");
    } else if let Some(local_file) = cli.put {
        let remote_file = cli.file
            .and_then(|p| p.to_str().map(String::from))
            .unwrap_or_else(|| local_file.clone());
        info!("Uploading {:?} to {} as {}", local_file, server_addr, remote_file);
        client.put(&PathBuf::from(&local_file), &remote_file).await?;
        info!("Upload complete");
    } else {
        return Err(TftpError::Tftp("Must specify either --get or --put".into()));
    }

    Ok(())
}

/// TFTP Client
struct TftpClient {
    server_addr: SocketAddr,
    mode: TransferMode,
    block_size: usize,
    timeout_secs: u64,
    windowsize: usize,
}

impl TftpClient {
    fn new(
        server_addr: SocketAddr,
        mode: TransferMode,
        block_size: usize,
        timeout_secs: u64,
        windowsize: usize,
    ) -> Result<Self> {
        Ok(Self {
            server_addr,
            mode,
            block_size,
            timeout_secs,
            windowsize,
        })
    }

    /// Download a file from the TFTP server (RRQ)
    async fn get(&mut self, remote_file: &str, local_file: &Path) -> Result<()> {
        // Create UDP socket with large receive buffer for RFC 7440 windowing
        // Calculate buffer size based on windowsize: windowsize * (block_size + 4 byte header) * 2 for safety
        let buffer_size_kb = ((self.windowsize * (self.block_size + 4) * 2) / 1024).max(512);

        let socket2_socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        socket2_socket.set_recv_buffer_size(buffer_size_kb * 1024)?;
        socket2_socket.set_nonblocking(true)?;
        socket2_socket.bind(&"0.0.0.0:0".parse::<SocketAddr>().unwrap().into())?;

        let std_socket: std::net::UdpSocket = socket2_socket.into();
        std_socket.set_nonblocking(true)?;
        let socket = UdpSocket::from_std(std_socket)?;

        debug!("Bound to {:?} with {}KB receive buffer", socket.local_addr()?, buffer_size_kb);

        // Send Read Request (RRQ) to server's listening port
        let mut packet = BytesMut::new();
        packet.put_u16(Opcode::Rrq as u16);
        packet.put(remote_file.as_bytes());
        packet.put_u8(0);
        packet.put(self.mode_str().as_bytes());
        packet.put_u8(0);

        // Add options
        if self.block_size != DEFAULT_BLOCK_SIZE {
            packet.put("blksize".as_bytes());
            packet.put_u8(0);
            packet.put(self.block_size.to_string().as_bytes());
            packet.put_u8(0);
        }

        if self.windowsize > 1 {
            packet.put("windowsize".as_bytes());
            packet.put_u8(0);
            packet.put(self.windowsize.to_string().as_bytes());
            packet.put_u8(0);
        }

        socket.send_to(&packet, self.server_addr).await?;
        debug!("Sent RRQ to {}", self.server_addr);

        // Create output file
        let mut file = File::create(local_file).await?;

        // Receive data with RFC 7440 windowing support
        let mut expected_block = 1u16;
        let mut total_bytes = 0usize;
        let start_time = std::time::Instant::now();
        let mut server_tid: Option<SocketAddr> = None; // Track server's transfer ID (port)

        // RFC 7440: Buffer for out-of-order blocks within the window
        use std::collections::HashMap;
        let mut block_buffer: HashMap<u16, Vec<u8>> = HashMap::new();
        let mut last_ack_sent = 0u16;

        loop {
            let mut buf = vec![0u8; self.block_size + 4];

            let (len, from_addr) = match timeout(
                Duration::from_secs(self.timeout_secs),
                socket.recv_from(&mut buf)
            ).await {
                Ok(Ok(result)) => result,
                Ok(Err(e)) => return Err(TftpError::Io(e)),
                Err(_) => return Err(TftpError::Tftp("Timeout waiting for data".into())),
            };

            // First packet establishes the server's TID (Transfer ID - its ephemeral port)
            if server_tid.is_none() {
                server_tid = Some(from_addr);
                debug!("Server TID: {}", from_addr);
            } else if Some(from_addr) != server_tid {
                // Ignore packets from other sources
                warn!("Ignoring packet from unexpected source: {}", from_addr);
                continue;
            }

            if len < 4 {
                return Err(TftpError::Tftp("Packet too small".into()));
            }

            let mut bytes = BytesMut::from(&buf[..len]);
            let opcode = bytes.get_u16();

            match Opcode::from_u16(opcode) {
                Some(Opcode::Data) => {
                    let block_num = bytes.get_u16();
                    let data = bytes.to_vec();
                    let data_len = data.len();

                    // RFC 7440: Handle out-of-order blocks
                    if block_num < expected_block {
                        // Duplicate block, ignore but send ACK
                        debug!("Duplicate block {} (already processed)", block_num);
                        self.send_ack_to(&socket, last_ack_sent, server_tid.unwrap()).await?;
                        continue;
                    } else if block_num > expected_block {
                        // Future block, buffer it
                        debug!("Buffering out-of-order block {} (expecting {})", block_num, expected_block);
                        block_buffer.insert(block_num, data);
                        continue;
                    }

                    // Write the expected block
                    file.write_all(&data).await?;
                    total_bytes += data_len;
                    debug!("Received block {} ({} bytes)", block_num, data_len);

                    let is_final = data_len < self.block_size;
                    expected_block = expected_block.wrapping_add(1);

                    // RFC 7440: Write any buffered blocks that are now in sequence
                    while let Some(buffered_data) = block_buffer.remove(&expected_block) {
                        let buffered_len = buffered_data.len();
                        file.write_all(&buffered_data).await?;
                        total_bytes += buffered_len;
                        debug!("Wrote buffered block {} ({} bytes)", expected_block, buffered_len);
                        expected_block = expected_block.wrapping_add(1);
                    }

                    // RFC 7440: Send ACK after receiving windowsize blocks or final block
                    // Calculate if we've completed a window boundary
                    let blocks_from_last_ack = expected_block.wrapping_sub(last_ack_sent.wrapping_add(1));
                    let should_ack = blocks_from_last_ack as usize >= self.windowsize || is_final;
                    if should_ack {
                        let ack_block = expected_block.wrapping_sub(1);
                        self.send_ack_to(&socket, ack_block, server_tid.unwrap()).await?;
                        debug!("Sent ACK for block {} (window complete: {} blocks from last ACK)", ack_block, blocks_from_last_ack);
                        last_ack_sent = ack_block;
                    }

                    // Check if this was the last block
                    if is_final {
                        info!("Transfer complete: {} bytes in {:.2}s",
                            total_bytes, start_time.elapsed().as_secs_f64());
                        break;
                    }
                }
                Some(Opcode::Error) => {
                    let error_code = bytes.get_u16();
                    let error_msg = String::from_utf8_lossy(&bytes).into_owned();
                    return Err(TftpError::Tftp(format!("Server error {}: {}", error_code, error_msg)));
                }
                Some(Opcode::Oack) => {
                    debug!("Received OACK, sending ACK 0");
                    self.send_ack_to(&socket, 0, server_tid.unwrap()).await?;
                    last_ack_sent = 0;
                }
                _ => {
                    return Err(TftpError::Tftp(format!("Unexpected opcode: {}", opcode)));
                }
            }
        }

        Ok(())
    }

    /// Upload a file to the TFTP server (WRQ)
    async fn put(&mut self, local_file: &Path, remote_file: &str) -> Result<()> {
        // Open local file
        let mut file = File::open(local_file).await?;
        let file_size = file.metadata().await?.len();

        // Create UDP socket with large receive buffer for ACKs
        let buffer_size_kb = 512; // 512KB should be sufficient for ACK packets

        let socket2_socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        socket2_socket.set_recv_buffer_size(buffer_size_kb * 1024)?;
        socket2_socket.set_nonblocking(true)?;
        socket2_socket.bind(&"0.0.0.0:0".parse::<SocketAddr>().unwrap().into())?;

        let std_socket: std::net::UdpSocket = socket2_socket.into();
        std_socket.set_nonblocking(true)?;
        std_socket.connect(self.server_addr)?;
        let socket = UdpSocket::from_std(std_socket)?;

        debug!("Connected to server on {:?} with {}KB receive buffer", socket.local_addr()?, buffer_size_kb);

        // Send Write Request (WRQ)
        self.send_wrq(&socket, remote_file, file_size).await?;

        // Wait for ACK 0 or OACK
        let mut buf = vec![0u8; 1024];
        let len = match timeout(
            Duration::from_secs(self.timeout_secs),
            socket.recv(&mut buf)
        ).await {
            Ok(Ok(len)) => len,
            Ok(Err(e)) => return Err(TftpError::Io(e)),
            Err(_) => return Err(TftpError::Tftp("Timeout waiting for ACK".into())),
        };

        let mut bytes = BytesMut::from(&buf[..len]);
        let opcode = bytes.get_u16();

        match Opcode::from_u16(opcode) {
            Some(Opcode::Ack) => {
                let block_num = bytes.get_u16();
                if block_num != 0 {
                    return Err(TftpError::Tftp(format!("Expected ACK 0, got {}", block_num)));
                }
            }
            Some(Opcode::Oack) => {
                debug!("Received OACK");
            }
            Some(Opcode::Error) => {
                let error_code = bytes.get_u16();
                let error_msg = String::from_utf8_lossy(&bytes).into_owned();
                return Err(TftpError::Tftp(format!("Server error {}: {}", error_code, error_msg)));
            }
            _ => {
                return Err(TftpError::Tftp(format!("Unexpected opcode: {}", opcode)));
            }
        }

        // Send data blocks
        let mut block_num = 1u16;
        let mut total_bytes = 0usize;
        let start_time = std::time::Instant::now();

        loop {
            let mut data = vec![0u8; self.block_size];
            let bytes_read = file.read(&mut data).await?;

            if bytes_read == 0 {
                break;
            }

            data.truncate(bytes_read);
            total_bytes += bytes_read;

            // Send DATA packet
            let mut retries = 0;
            loop {
                self.send_data(&socket, block_num, &data).await?;

                // Wait for ACK
                let mut buf = vec![0u8; 1024];
                match timeout(
                    Duration::from_secs(self.timeout_secs),
                    socket.recv(&mut buf)
                ).await {
                    Ok(Ok(len)) => {
                        let mut bytes = BytesMut::from(&buf[..len]);
                        let opcode = bytes.get_u16();

                        match Opcode::from_u16(opcode) {
                            Some(Opcode::Ack) => {
                                let ack_block = bytes.get_u16();
                                if ack_block == block_num {
                                    debug!("Received ACK for block {}", block_num);
                                    break;
                                }
                            }
                            Some(Opcode::Error) => {
                                let error_code = bytes.get_u16();
                                let error_msg = String::from_utf8_lossy(&bytes).into_owned();
                                return Err(TftpError::Tftp(format!("Server error {}: {}", error_code, error_msg)));
                            }
                            _ => {}
                        }
                    }
                    Ok(Err(e)) => return Err(TftpError::Io(e)),
                    Err(_) => {
                        retries += 1;
                        if retries >= MAX_RETRIES {
                            return Err(TftpError::Tftp("Max retries exceeded".into()));
                        }
                        warn!("Timeout waiting for ACK {}, retrying ({}/{})", block_num, retries, MAX_RETRIES);
                    }
                }
            }

            if bytes_read < self.block_size {
                info!("Transfer complete: {} bytes in {:.2}s",
                    total_bytes, start_time.elapsed().as_secs_f64());
                break;
            }

            block_num = block_num.wrapping_add(1);
        }

        Ok(())
    }

    /// Send Write Request (WRQ)
    async fn send_wrq(&self, socket: &UdpSocket, filename: &str, file_size: u64) -> Result<()> {
        let mut packet = BytesMut::new();
        packet.put_u16(Opcode::Wrq as u16);
        packet.put(filename.as_bytes());
        packet.put_u8(0);
        packet.put(self.mode_str().as_bytes());
        packet.put_u8(0);

        // Add options
        if self.block_size != DEFAULT_BLOCK_SIZE {
            packet.put("blksize".as_bytes());
            packet.put_u8(0);
            packet.put(self.block_size.to_string().as_bytes());
            packet.put_u8(0);
        }

        if self.windowsize > 1 {
            packet.put("windowsize".as_bytes());
            packet.put_u8(0);
            packet.put(self.windowsize.to_string().as_bytes());
            packet.put_u8(0);
        }

        // Add transfer size
        packet.put("tsize".as_bytes());
        packet.put_u8(0);
        packet.put(file_size.to_string().as_bytes());
        packet.put_u8(0);

        socket.send(&packet).await?;
        Ok(())
    }

    /// Send ACK packet
    async fn send_ack(&self, socket: &UdpSocket, block_num: u16) -> Result<()> {
        let mut packet = BytesMut::with_capacity(4);
        packet.put_u16(Opcode::Ack as u16);
        packet.put_u16(block_num);
        socket.send(&packet).await?;
        Ok(())
    }

    /// Send ACK packet to specific address
    async fn send_ack_to(&self, socket: &UdpSocket, block_num: u16, addr: SocketAddr) -> Result<()> {
        let mut packet = BytesMut::with_capacity(4);
        packet.put_u16(Opcode::Ack as u16);
        packet.put_u16(block_num);
        socket.send_to(&packet, addr).await?;
        Ok(())
    }

    /// Send DATA packet
    async fn send_data(&self, socket: &UdpSocket, block_num: u16, data: &[u8]) -> Result<()> {
        let mut packet = BytesMut::with_capacity(4 + data.len());
        packet.put_u16(Opcode::Data as u16);
        packet.put_u16(block_num);
        packet.put(data);
        socket.send(&packet).await?;
        Ok(())
    }

    /// Get mode as string
    fn mode_str(&self) -> &'static str {
        match self.mode {
            TransferMode::Netascii => "netascii",
            TransferMode::Octet => "octet",
            TransferMode::Mail => "mail",
        }
    }
}
