//! 测试专用 mock 上游：验证拨号器实际发出的认证字节流。
//!
//! 仅在测试构建中编译（声明处 `#[cfg(test)]`）。
//! HTTP mock 要求 Basic 认证，凭据不符回 407；SOCKS5 mock 按 RFC 1928/1929
//! 完整走方法协商与子协商，客户端只提供无认证时按规范回 0xFF。

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// 启动要求 Basic 认证的 mock HTTP CONNECT 代理，返回监听地址。
/// 认证通过后对隧道内的 HTTP 请求统一回 204。
pub async fn http_proxy(expect_token: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let Some(head) = read_until_head_end(&mut s).await else {
                    return;
                };
                if head.contains(&format!("Proxy-Authorization: Basic {expect_token}")) {
                    s.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                        .await
                        .ok();
                    let _ = read_until_head_end(&mut s).await; // 隧道内探测请求
                    s.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                        .await
                        .ok();
                } else {
                    s.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n")
                        .await
                        .ok();
                }
            });
        }
    });
    addr
}

/// 启动要求用户名/密码认证的 mock SOCKS5 代理，返回监听地址。
pub async fn socks5_proxy(user: &'static str, pass: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut head = [0u8; 2];
                if s.read_exact(&mut head).await.is_err() {
                    return;
                }
                let mut methods = vec![0u8; head[1] as usize];
                if s.read_exact(&mut methods).await.is_err() {
                    return;
                }
                if !methods.contains(&0x02) {
                    // RFC 1928：客户端未提供可接受的方法 → 0xFF 并断开
                    s.write_all(&[0x05, 0xFF]).await.ok();
                    return;
                }
                s.write_all(&[0x05, 0x02]).await.ok();
                // RFC 1929 子协商：VER ULEN UNAME PLEN PASSWD
                let mut ver = [0u8; 1];
                if s.read_exact(&mut ver).await.is_err() {
                    return;
                }
                let mut ulen = [0u8; 1];
                if s.read_exact(&mut ulen).await.is_err() {
                    return;
                }
                let mut uname = vec![0u8; ulen[0] as usize];
                if s.read_exact(&mut uname).await.is_err() {
                    return;
                }
                let mut plen = [0u8; 1];
                if s.read_exact(&mut plen).await.is_err() {
                    return;
                }
                let mut passwd = vec![0u8; plen[0] as usize];
                if s.read_exact(&mut passwd).await.is_err() {
                    return;
                }
                let ok = uname == user.as_bytes() && passwd == pass.as_bytes();
                s.write_all(&[0x01, if ok { 0x00 } else { 0x01 }])
                    .await
                    .ok();
                if !ok {
                    return;
                }
                // CONNECT：VER CMD RSV ATYP + 地址 + 端口
                let mut req = [0u8; 4];
                if s.read_exact(&mut req).await.is_err() {
                    return;
                }
                let skip = match req[3] {
                    0x01 => 4,
                    0x04 => 16,
                    0x03 => {
                        let mut l = [0u8; 1];
                        if s.read_exact(&mut l).await.is_err() {
                            return;
                        }
                        l[0] as usize
                    }
                    _ => return,
                };
                let mut rest = vec![0u8; skip + 2];
                if s.read_exact(&mut rest).await.is_err() {
                    return;
                }
                s.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                    .await
                    .ok();
                let _ = read_until_head_end(&mut s).await;
                s.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                    .await
                    .ok();
            });
        }
    });
    addr
}

async fn read_until_head_end(s: &mut tokio::net::TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 512];
    loop {
        let n = s.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            return Some(String::from_utf8_lossy(&buf).into_owned());
        }
    }
}
