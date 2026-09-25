//! 入站 HTTP 代理处理：CONNECT 隧道与纯 HTTP 请求的改写转发。

use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use super::router;
use super::stream::read_head;
use super::upstream;
use super::url::{authority_path, split_host_port};
use super::{EngineCtx, LogLevel};

const BAD_REQUEST: &[u8] =
    b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const BAD_GATEWAY: &[u8] =
    b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const CONNECTION_ESTABLISHED: &[u8] = b"HTTP/1.1 200 Connection Established\r\n\r\n";

pub struct Header {
    pub raw: String,
    pub lc: String,
    pub value: String,
}

pub struct Request {
    pub method: String,
    pub target: String,
    pub headers: Vec<Header>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        let lc = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|h| h.lc == lc)
            .map(|h| h.value.as_str())
    }
}

fn parse_request(head: &[u8]) -> Result<Request, String> {
    let text = std::str::from_utf8(head).map_err(|_| "头部不是 UTF-8".to_string())?;
    let mut lines = text.split("\r\n");
    let req_line = lines.next().ok_or("空请求")?;
    let mut parts = req_line.split_whitespace();
    let method = parts.next().ok_or("缺少 method")?.to_string();
    let target = parts.next().ok_or("缺少 target")?.to_string();
    let version = parts.next().unwrap_or("HTTP/1.1").to_string();
    if !version.to_ascii_uppercase().starts_with("HTTP/") {
        return Err(format!("非 HTTP 请求行: {req_line}"));
    }
    let mut headers = Vec::new();
    for l in lines {
        if l.is_empty() {
            continue;
        }
        if let Some(c) = l.find(':') {
            let raw = l[..c].trim().to_string();
            let value = l[c + 1..].trim().to_string();
            headers.push(Header {
                lc: raw.to_ascii_lowercase(),
                raw,
                value,
            });
        }
    }
    Ok(Request {
        method,
        target,
        headers,
    })
}

pub async fn handle(mut stream: TcpStream, ctx: Arc<EngineCtx>) {
    stream.set_nodelay(true).ok();
    let (head_bytes, leftover) = match tokio::time::timeout(
        Duration::from_secs(30),
        read_head(&mut stream, 64 * 1024),
    )
    .await
    {
        Ok(Ok(h)) => h,
        Ok(Err(_)) => return,
        Err(_) => return,
    };
    let req = match parse_request(&head_bytes) {
        Ok(r) => r,
        Err(_) => {
            let _ = stream.write_all(BAD_REQUEST).await;
            return;
        }
    };
    if req.method.eq_ignore_ascii_case("CONNECT") {
        handle_connect(stream, ctx, req, leftover).await;
    } else {
        handle_plain(stream, ctx, req, leftover).await;
    }
}

async fn handle_connect(
    mut stream: TcpStream,
    ctx: Arc<EngineCtx>,
    req: Request,
    leftover: Vec<u8>,
) {
    let authority = req.target.clone();
    let Some((host, port)) = split_host_port(&authority, 443) else {
        let _ = stream.write_all(BAD_REQUEST).await;
        return;
    };
    match router::connect_target(&ctx, "CONNECT", &host, port).await {
        Ok((idx, mut up)) => {
            if stream.write_all(CONNECTION_ESTABLISHED).await.is_err() {
                return;
            }
            // 客户端可能把首段数据（如 TLS ClientHello）和 CONNECT 头一起发出
            if !leftover.is_empty() && up.write_all(&leftover).await.is_err() {
                return;
            }
            if let Ok((up_bytes, down_bytes)) =
                tokio::io::copy_bidirectional(&mut stream, &mut up).await
            {
                ctx.state.record_traffic(idx, up_bytes, down_bytes);
            }
        }
        Err(e) => {
            ctx.log(LogLevel::Warn, format!("CONNECT {host}:{port} 失败: {e}"));
            let _ = stream.write_all(BAD_GATEWAY).await;
        }
    }
}

async fn handle_plain(mut stream: TcpStream, ctx: Arc<EngineCtx>, req: Request, leftover: Vec<u8>) {
    let target = resolve_plain_target(&req);
    let Some((host, port, path)) = target else {
        let _ = stream.write_all(BAD_REQUEST).await;
        return;
    };

    let upgrade = req.header("upgrade").is_some();
    match router::connect_target(&ctx, &req.method, &host, port).await {
        Ok((idx, mut up)) => {
            // Proxy-Authorization 逐跳生效：拿到上游后才能按其类型补凭据
            let proxy_auth = upstream::proxy_auth_header(&ctx, idx).await;
            let new_head = build_origin_head(
                &req,
                &path,
                &format!("{host}:{port}"),
                upgrade,
                proxy_auth.as_deref(),
            );
            let mut payload = new_head.into_bytes();
            payload.extend_from_slice(&leftover);
            if up.write_all(&payload).await.is_err() {
                return;
            }
            if let Ok((up_bytes, down_bytes)) =
                tokio::io::copy_bidirectional(&mut stream, &mut up).await
            {
                ctx.state.record_traffic(idx, up_bytes, down_bytes);
            }
        }
        Err(e) => {
            ctx.log(
                LogLevel::Warn,
                format!("{} {host}:{port} 失败: {e}", req.method),
            );
            let _ = stream.write_all(BAD_GATEWAY).await;
        }
    }
}

/// 解析纯 HTTP 请求的目标：绝对 URI（改写为 origin-form）或 origin-form + Host 头。
fn resolve_plain_target(req: &Request) -> Option<(String, u16, String)> {
    if let Some(rest) = req.target.strip_prefix("http://") {
        let (auth, path) = authority_path(rest)?;
        let (h, p) = split_host_port(auth, 80)?;
        return Some((h, p, path));
    }
    if req.target.starts_with('/') {
        let host = req.header("host")?;
        let (h, p) = split_host_port(host, 80)?;
        return Some((h, p, req.target.clone()));
    }
    None
}

/// 把客户端请求头改写为发往 origin 的形式：
/// - 绝对 URI → origin-form 请求行；
/// - 剥离 hop-by-hop 头（Connection 列名 + Proxy-Connection/Proxy-Authorization/TE/Trailer/Keep-Alive）；
/// - 强制 `Connection: close`（Upgrade 请求除外，按隧道原样保留）；
/// - Host 缺失时从 authority 合成；
/// - `proxy_auth` 为上游这一跳的凭据（`Basic xxx`），仅 HTTP 型上游由调用方提供。
fn build_origin_head(
    req: &Request,
    path: &str,
    authority: &str,
    upgrade: bool,
    proxy_auth: Option<&str>,
) -> String {
    let mut conn_tokens: Vec<String> = Vec::new();
    for h in &req.headers {
        if h.lc == "connection" {
            for t in h.value.split(',') {
                let t = t.trim().to_ascii_lowercase();
                if !t.is_empty() {
                    conn_tokens.push(t);
                }
            }
        }
    }

    let mut out = String::with_capacity(256);
    out.push_str(&format!("{} {} HTTP/1.1\r\n", req.method, path));
    let mut host_seen = false;
    for h in &req.headers {
        if h.lc == "host" {
            host_seen = true;
            out.push_str(&format!("Host: {}\r\n", h.value));
            continue;
        }
        let always_drop = matches!(
            h.lc.as_str(),
            "proxy-connection" | "proxy-authorization" | "te" | "trailer" | "keep-alive"
        );
        if always_drop {
            continue;
        }
        if h.lc == "connection" {
            if upgrade {
                out.push_str(&format!("Connection: {}\r\n", h.value));
            }
            continue;
        }
        // Upgrade 请求（WebSocket 等）需要把 Connection/Upgrade 原样带到 origin
        if !upgrade && conn_tokens.contains(&h.lc) {
            continue;
        }
        out.push_str(&format!("{}: {}\r\n", h.raw, h.value));
    }
    if !host_seen {
        out.push_str(&format!("Host: {authority}\r\n"));
    }
    if let Some(v) = proxy_auth {
        out.push_str(&format!("Proxy-Authorization: {v}\r\n"));
    }
    if !upgrade {
        out.push_str("Connection: close\r\n");
    }
    out.push_str("\r\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(head: &str) -> Request {
        parse_request(head.as_bytes()).unwrap()
    }

    #[test]
    fn parses_request_line_and_headers() {
        let req = request("GET http://a.com/x HTTP/1.1\r\nHost: a.com\r\nAccept: */*\r\n\r\n");
        assert_eq!(req.method, "GET");
        assert_eq!(req.target, "http://a.com/x");
        assert_eq!(req.header("host"), Some("a.com"));
        assert_eq!(req.header("ACCEPT"), Some("*/*"));
        assert_eq!(req.headers.len(), 2);
    }

    #[test]
    fn rejects_non_http_request_lines() {
        assert!(parse_request(b"\x16\x03\x01garbage\r\n\r\n").is_err());
    }

    #[test]
    fn rewrites_to_origin_form_and_forces_close() {
        let req = request(
            "GET http://example.com/path?q=1 HTTP/1.1\r\nHost: example.com\r\nProxy-Authorization: Basic zz\r\nProxy-Connection: keep-alive\r\nAccept: */*\r\n\r\n",
        );
        let head = build_origin_head(&req, "/path?q=1", "example.com:80", false, None);
        assert!(head.starts_with("GET /path?q=1 HTTP/1.1\r\n"));
        assert!(head.contains("Host: example.com\r\n"));
        assert!(head.contains("Connection: close\r\n"));
        assert!(head.contains("Accept: */*\r\n"));
        assert!(!head.contains("Proxy-"));
        assert!(head.ends_with("\r\n\r\n"));
    }

    #[test]
    fn adds_upstream_proxy_credentials() {
        let req = request("GET http://example.com/ HTTP/1.1\r\n\r\n");
        let head = build_origin_head(
            &req,
            "/",
            "example.com:80",
            false,
            Some("Basic dXNlcjpwYXNz"),
        );
        assert!(head.contains("Proxy-Authorization: Basic dXNlcjpwYXNz\r\n"));
        assert!(head.ends_with("\r\n\r\n"));
    }

    #[test]
    fn synthesizes_host_when_missing() {
        let req = request("GET http://example.com/ HTTP/1.1\r\n\r\n");
        let head = build_origin_head(&req, "/", "example.com:80", false, None);
        assert!(head.contains("Host: example.com:80\r\n"));
    }

    #[test]
    fn keeps_upgrade_semantics_for_websocket() {
        let req = request(
            "GET ws://example.com/chat HTTP/1.1\r\nHost: example.com\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n",
        );
        let head = build_origin_head(&req, "/chat", "example.com:80", true, None);
        assert!(head.contains("Connection: Upgrade\r\n"));
        assert!(head.contains("Upgrade: websocket\r\n"));
        assert!(!head.contains("Connection: close"));
    }

    #[test]
    fn strips_connection_listed_fields() {
        let req = request(
            "GET / HTTP/1.1\r\nHost: a.com\r\nConnection: X-Custom, close\r\nX-Custom: v\r\n\r\n",
        );
        let head = build_origin_head(&req, "/", "a.com:80", false, None);
        assert!(!head.contains("X-Custom"));
        assert!(head.contains("Connection: close\r\n"));
    }
}
