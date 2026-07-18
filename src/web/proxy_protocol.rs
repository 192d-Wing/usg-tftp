use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tracing::warn;

const V2_SIGNATURE: &[u8; 12] = b"\r\n\r\n\x00\r\nQUIT\n";
const V1_PREFIX: &[u8; 6] = b"PROXY ";
const HEADER_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_V2_ADDR_LEN: usize = 512;

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
    let mut peek = [0u8; 12];
    loop {
        let n = stream.peek(&mut peek).await.ok()?;
        if n == 0 {
            return None;
        }
        if n >= 12 {
            break;
        }
        tokio::task::yield_now().await;
    }

    if &peek == V2_SIGNATURE {
        read_v2(stream).await
    } else if peek[..6] == *V1_PREFIX {
        read_v1(stream).await
    } else {
        warn!("PROXY protocol enabled but no valid header received");
        None
    }
}

async fn read_v2(stream: &mut TcpStream) -> Option<SocketAddr> {
    let mut header = [0u8; 16];
    stream.read_exact(&mut header).await.ok()?;

    let ver_cmd = header[12];
    let fam_proto = header[13];
    let len = u16::from_be_bytes([header[14], header[15]]) as usize;

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
            // IPv4: 4+4 bytes addrs + 2+2 bytes ports = 12
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
            // IPv6: 16+16 bytes addrs + 2+2 bytes ports = 36
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

async fn read_v1(stream: &mut TcpStream) -> Option<SocketAddr> {
    let mut buf = Vec::with_capacity(108);
    loop {
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte).await.ok()?;
        buf.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
        if buf.len() > 107 {
            warn!("PROXY protocol v1 header too long");
            return None;
        }
    }

    let line = String::from_utf8_lossy(&buf);
    let parts: Vec<&str> = line.trim().split(' ').collect();
    // PROXY TCP4/TCP6 src_addr dst_addr src_port dst_port
    if parts.len() < 6 {
        return None;
    }

    let src_ip: IpAddr = parts[2].parse().ok()?;
    let src_port: u16 = parts[4].parse().ok()?;
    Some(SocketAddr::new(src_ip, src_port))
}
