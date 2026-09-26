//! 入站监听：接受 TCP 连接，按首字节嗅探协议并分发到对应处理器。

use std::sync::Arc;
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use super::http;
use super::socks5;
use super::state::{EngineCtx, LogLevel};

pub async fn run(listener: TcpListener, ctx: Arc<EngineCtx>, mut stop: watch::Receiver<bool>) {
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            r = listener.accept() => match r {
                Ok((s, _peer)) => {
                    let ctx = ctx.clone();
                    tokio::spawn(handle(s, ctx));
                }
                Err(e) => {
                    ctx.log(LogLevel::Error, format!("accept 失败: {e}"));
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }
    ctx.log(LogLevel::Info, "监听循环已退出");
}

/// 首字节嗅探：0x05 → SOCKS5；0x04 → SOCKS4（明确拒绝）；其余 → HTTP/1.1。
async fn handle(stream: TcpStream, ctx: Arc<EngineCtx>) {
    let mut first = [0u8; 1];
    let n = match stream.peek(&mut first).await {
        Ok(n) => n,
        Err(_) => return,
    };
    if n == 0 {
        return;
    }
    match first[0] {
        0x05 => socks5::handle(stream, ctx).await,
        0x04 => {
            ctx.log(
                LogLevel::Warn,
                "收到 SOCKS4 请求，已拒绝（仅支持 SOCKS5/HTTP）",
            );
        }
        _ => http::handle(stream, ctx).await,
    }
}
