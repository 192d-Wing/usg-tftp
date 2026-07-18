use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tracing::warn;

const V2_SIGNATURE: &[u8; 12] = b"\r\n\r\n\x00\r\nQUIT\n";
const V1_PREFIX: &[u8; 6] = b"PROXY ";

pub async fn read_proxy_header(stream: &mut TcpStream) -> Option<SocketAddr> {
    let mut peek = [0u8; 12];
    if stream.peek(&mut peek).await.ok()? < 12 {
        return None;
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
        let mut discard = vec![0u8; len];
        let _ = stream.read_exact(&mut discard).await;
        return None;
    }

    let command = ver_cmd & 0x0F;
    if command == 0 {
        // LOCAL command — health check, no real address
        let mut discard = vec![0u8; len];
        let _ = stream.read_exact(&mut discard).await;
        return None;
    }

    let addr_family = fam_proto >> 4;
    match addr_family {
        1 => {
            // IPv4
            if len < 12 {
                let mut discard = vec![0u8; len];
                let _ = stream.read_exact(&mut discard).await;
                return None;
            }
            let mut addr_data = vec![0u8; len];
            stream.read_exact(&mut addr_data).await.ok()?;
            let src_ip = Ipv4Addr::new(addr_data[0], addr_data[1], addr_data[2], addr_data[3]);
            let src_port = u16::from_be_bytes([addr_data[8], addr_data[9]]);
            Some(SocketAddr::new(IpAddr::V4(src_ip), src_port))
        }
        2 => {
            // IPv6
            if len < 36 {
                let mut discard = vec![0u8; len];
                let _ = stream.read_exact(&mut discard).await;
                return None;
            }
            let mut addr_data = vec![0u8; len];
            stream.read_exact(&mut addr_data).await.ok()?;
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&addr_data[..16]);
            let src_ip = Ipv6Addr::from(octets);
            let src_port = u16::from_be_bytes([addr_data[32], addr_data[33]]);
            Some(SocketAddr::new(IpAddr::V6(src_ip), src_port))
        }
        _ => {
            let mut discard = vec![0u8; len];
            let _ = stream.read_exact(&mut discard).await;
            None
        }
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
