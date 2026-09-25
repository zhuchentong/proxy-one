use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::router;
use super::{EngineCtx, LogLevel};

async fn socks_reply(stream: &mut TcpStream, code: u8) {
    let _ = stream
        .write_all(&[0x05, code, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await;
}

pub async fn handle(mut stream: TcpStream, ctx: Arc<EngineCtx>) {
    stream.set_nodelay(true).ok();
    let mut h = [0u8; 2];
    if stream.read_exact(&mut h).await.is_err() {
        return;
    }
    if h[0] != 0x05 {
        return;
    }
    let n = h[1] as usize;
    if n == 0 {
        return;
    }
    let mut methods = vec![0u8; n];
    if stream.read_exact(&mut methods).await.is_err() {
        return;
    }
    if !methods.contains(&0x00) {
        let _ = stream.write_all(&[0x05, 0xFF]).await;
        return;
    }
    if stream.write_all(&[0x05, 0x00]).await.is_err() {
        return;
    }

    let mut req = [0u8; 4];
    if stream.read_exact(&mut req).await.is_err() {
        return;
    }
    let (ver, cmd, _rsv, atyp) = (req[0], req[1], req[2], req[3]);
    if ver != 0x05 {
        return;
    }
    if atyp != 0x01 && atyp != 0x03 && atyp != 0x04 {
        socks_reply(&mut stream, 0x08).await;
        return;
    }
    let host = match atyp {
        0x01 => {
            let mut b = [0u8; 4];
            if stream.read_exact(&mut b).await.is_err() {
                return;
            }
            std::net::Ipv4Addr::from(b).to_string()
        }
        0x04 => {
            let mut b = [0u8; 16];
            if stream.read_exact(&mut b).await.is_err() {
                return;
            }
            std::net::Ipv6Addr::from(b).to_string()
        }
        _ => {
            let mut l = [0u8; 1];
            if stream.read_exact(&mut l).await.is_err() {
                return;
            }
            let mut b = vec![0u8; l[0] as usize];
            if stream.read_exact(&mut b).await.is_err() {
                return;
            }
            String::from_utf8_lossy(&b).into_owned()
        }
    };
    let mut pb = [0u8; 2];
    if stream.read_exact(&mut pb).await.is_err() {
        return;
    }
    let port = u16::from_be_bytes(pb);

    if cmd != 0x01 {
        ctx.log(
            LogLevel::Warn,
            format!("SOCKS5 不支持的命令 {cmd:#04x}（仅支持 CONNECT）"),
        );
        socks_reply(&mut stream, 0x07).await;
        return;
    }

    match router::connect_target(&ctx, "SOCKS5", &host, port).await {
        Ok((idx, mut up)) => {
            let _ = stream
                .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await;
            if let Ok((up_bytes, down_bytes)) =
                tokio::io::copy_bidirectional(&mut stream, &mut up).await
            {
                ctx.state.record_traffic(idx, up_bytes, down_bytes);
            }
        }
        Err(e) => {
            ctx.log(LogLevel::Warn, format!("SOCKS5 {host}:{port} 失败: {e}"));
            socks_reply(&mut stream, 0x05).await;
        }
    }
}
