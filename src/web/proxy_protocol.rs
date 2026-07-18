use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tracing::warn;

const V2_SIGNATURE: &[u8; 12] = b"\r\n\r\n\x00\r\nQUIT\n";
const V1_PREFIX: &[u8; 6] = b"PROXY ";
const HEADER_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_V2_ADDR_LEN: usize = 512;

/// Read and parse a PROXY protocol header from the stream.
///
/// Returns `Some(addr)` on success. Returns `None` if parsing fails or times out —
/// in either case bytes have been consumed and the caller MUST close the connection.
pub async fn read_proxy_header(stream: &mut TcpStream) -> Option<SocketAddr> {
    match tokio::time::timeout(HEADER_TIMEOUT, read_proxy_header_inner(stream)).await {
        Ok(result) => result,
        Err(_) => {
            warn!("PROXY protocol header read timed out");
            None
        }
    }
}

async fn read_proxy_header_inner(stream: &mut TcpStream) -> Option<SocketAddr> {
    let mut initial = [0u8; 12];
    stream.read_exact(&mut initial).await.ok()?;

    if initial == *V2_SIGNATURE {
        read_v2(stream).await
    } else if initial[..6] == *V1_PREFIX {
        read_v1(stream, &initial).await
    } else {
        warn!("PROXY protocol enabled but no valid header received");
        None
    }
}

async fn read_v2(stream: &mut TcpStream) -> Option<SocketAddr> {
    let mut rest = [0u8; 4];
    stream.read_exact(&mut rest).await.ok()?;

    let ver_cmd = rest[0];
    let fam_proto = rest[1];
    let len = u16::from_be_bytes([rest[2], rest[3]]) as usize;

    let version = ver_cmd >> 4;
    if version != 2 {
        warn!("Unsupported PROXY protocol version: {}", version);
        discard(stream, len).await;
        return None;
    }

    if len > MAX_V2_ADDR_LEN {
        warn!("PROXY protocol v2 address length too large: {}", len);
        discard(stream, len).await;
        return None;
    }

    let command = ver_cmd & 0x0F;
    if command != 1 {
        discard(stream, len).await;
        return None;
    }

    let addr_family = fam_proto >> 4;
    match addr_family {
        1 => {
            if len < 12 {
                discard(stream, len).await;
                return None;
            }
            let mut addr_data = [0u8; 12];
            stream.read_exact(&mut addr_data).await.ok()?;
            if len > 12 {
                discard(stream, len - 12).await;
            }
            let src_ip = Ipv4Addr::new(addr_data[0], addr_data[1], addr_data[2], addr_data[3]);
            let src_port = u16::from_be_bytes([addr_data[8], addr_data[9]]);
            Some(SocketAddr::new(IpAddr::V4(src_ip), src_port))
        }
        2 => {
            if len < 36 {
                discard(stream, len).await;
                return None;
            }
            let mut addr_data = [0u8; 36];
            stream.read_exact(&mut addr_data).await.ok()?;
            if len > 36 {
                discard(stream, len - 36).await;
            }
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&addr_data[..16]);
            let src_ip = Ipv6Addr::from(octets);
            let src_port = u16::from_be_bytes([addr_data[32], addr_data[33]]);
            Some(SocketAddr::new(IpAddr::V6(src_ip), src_port))
        }
        _ => {
            discard(stream, len).await;
            None
        }
    }
}

async fn discard(stream: &mut TcpStream, len: usize) {
    let mut remaining = len;
    let mut buf = [0u8; 256];
    while remaining > 0 {
        let to_read = remaining.min(buf.len());
        if stream.read_exact(&mut buf[..to_read]).await.is_err() {
            break;
        }
        remaining -= to_read;
    }
}

async fn read_v1(stream: &mut TcpStream, initial: &[u8; 12]) -> Option<SocketAddr> {
    let mut header = Vec::with_capacity(108);
    header.extend_from_slice(initial);

    // Optimistic path: peek for the rest of the line in one shot.
    // PROXY v1 headers arrive atomically in practice, so this almost always succeeds.
    let mut peek_buf = [0u8; 96];
    let n = stream.peek(&mut peek_buf).await.ok()?;
    if n == 0 {
        return None;
    }

    if let Some(pos) = peek_buf[..n].iter().position(|&b| b == b'\n') {
        let to_read = pos + 1;
        let mut buf = vec![0u8; to_read];
        stream.read_exact(&mut buf).await.ok()?;
        header.extend_from_slice(&buf);
    } else if header.len() + n > 107 {
        warn!("PROXY protocol v1 header too long");
        return None;
    } else {
        // Rare fragmentation: fall back to byte-by-byte (properly awaits readiness)
        loop {
            let mut byte = [0u8; 1];
            stream.read_exact(&mut byte).await.ok()?;
            header.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
            if header.len() > 107 {
                warn!("PROXY protocol v1 header too long");
                return None;
            }
        }
    }

    let line = String::from_utf8_lossy(&header);
    let parts: Vec<&str> = line.trim().split(' ').collect();
    if parts.len() < 6 {
        return None;
    }

    let src_ip: IpAddr = parts[2].parse().ok()?;
    let src_port: u16 = parts[4].parse().ok()?;
    Some(SocketAddr::new(src_ip, src_port))
}
