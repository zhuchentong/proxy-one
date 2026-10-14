//! 入站 HTTP 代理处理：CONNECT 隧道与纯 HTTP 请求的改写转发。

use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use super::router;
use super::state::EngineCtx;
use super::state::LogLevel;
use super::stream::{CountingStream, noop_callbacks, read_head, traffic_callbacks};
use super::upstream;
use super::url::{authority_path, split_host_port};

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
        Ok((route, up)) => {
            // 上游流包一层实时记账：转发字节随传输入账（含 CONNECT 首段的 leftover）；
            // 直连不经上游，无归属即不记账
            let (on_write, on_read) = match route {
                router::Route::Upstream(idx) => traffic_callbacks(ctx.state.clone(), idx),
                router::Route::Direct => noop_callbacks(),
            };
            let mut up = CountingStream::new(up, on_write, on_read);
            if stream.write_all(CONNECTION_ESTABLISHED).await.is_err() {
                return;
            }
            // 客户端可能把首段数据（如 TLS ClientHello）和 CONNECT 头一起发出
            if !leftover.is_empty() && up.write_all(&leftover).await.is_err() {
                router::note_upstream_failure(&ctx, route, true);
                return;
            }
            let res = tokio::io::copy_bidirectional(&mut stream, &mut up).await;
            router::note_upstream_failure(&ctx, route, res.is_err());
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
        Ok((route, up)) => {
            // 同 CONNECT：包装流实时记账，改写后的请求头也是上行流量
            let (on_write, on_read) = match route {
                router::Route::Upstream(idx) => traffic_callbacks(ctx.state.clone(), idx),
                router::Route::Direct => noop_callbacks(),
            };
            let mut up = CountingStream::new(up, on_write, on_read);
            // Proxy-Authorization 逐跳生效：拿到上游后才能按其类型补凭据；直连无需代理凭据
            let proxy_auth = match route {
                router::Route::Upstream(idx) => upstream::proxy_auth_header(&ctx, idx).await,
                router::Route::Direct => None,
            };
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
                router::note_upstream_failure(&ctx, route, true);
                return;
            }
            let res = tokio::io::copy_bidirectional(&mut stream, &mut up).await;
            router::note_upstream_failure(&ctx, route, res.is_err());
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

    use crate::config::{Config, UpstreamConfig, UpstreamKind};
    use crate::engine::state::StateStore;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn request(head: &str) -> Request {
        parse_request(head.as_bytes()).unwrap()
    }

    /// 构造仅含一个指定 mock 上游的引擎上下文，返回 (ctx, 健康触发接收端)。
    fn ctx_with_mock(addr: String) -> (Arc<EngineCtx>, tokio::sync::mpsc::UnboundedReceiver<()>) {
        let cfg = Config {
            upstreams: vec![UpstreamConfig {
                name: "mock".into(),
                addr,
                kind: UpstreamKind::Http,
                priority: 1,
                username: None,
                password: None,
            }],
            ..Config::default()
        };
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx = Arc::new(EngineCtx {
            cfg: Arc::new(cfg.clone()),
            state: Arc::new(StateStore::new(&cfg)),
            health_tx: tx,
            kind_cache: Mutex::new(HashMap::new()),
            url_warned: AtomicBool::new(false),
        });
        (ctx, rx)
    }

    /// 与 mock 上游建立 CONNECT 隧道，返回 (客户端流, 转发任务)。
    async fn tunnel_via_mock(ctx: Arc<EngineCtx>) -> (TcpStream, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let req = request("CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n");
        let task = tokio::spawn(handle_connect(server, ctx, req, Vec::new()));
        // 等隧道建立应答，确保 copy_bidirectional 已启动
        let mut buf = vec![0u8; 256];
        let _ = client.read(&mut buf).await;
        (client, task)
    }

    #[tokio::test]
    async fn upstream_tunnel_failure_triggers_health_round() {
        let mock = upstream::mock::http_proxy_drop_after_reply().await;
        let (ctx, mut rx) = ctx_with_mock(mock);
        let (mut client, task) = tunnel_via_mock(ctx).await;

        // 隧道内写两段数据：mock 已 drop，第一段可能进缓冲成功，第二段写入时
        // RST 已置位 → copy_bidirectional 必以 Err 结束 → 应触发健康检查
        client.write_all(b"hello-1").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let _ = client.write_all(b"hello-2").await;
        let triggered = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await;
        assert!(
            matches!(triggered, Ok(Some(()))),
            "上游隧道失败应触发一轮健康检查"
        );
        task.abort();
    }

    #[tokio::test]
    async fn direct_tunnel_failure_does_not_trigger_health_round() {
        // 直连目标：accept 后立即 drop（无上游参与）
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = listener.local_addr().unwrap().to_string();
        let mut cfg = Config::default();
        cfg.upstreams.clear();
        cfg.general.bypass_hosts = vec![target.clone()];
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx = Arc::new(EngineCtx {
            cfg: Arc::new(cfg.clone()),
            state: Arc::new(StateStore::new(&cfg)),
            health_tx: tx,
            kind_cache: Mutex::new(HashMap::new()),
            url_warned: AtomicBool::new(false),
        });

        let client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let req = request(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_str());
        let task = tokio::spawn(handle_connect(server, ctx, req, Vec::new()));
        // 等客户端到目标的直连在引擎侧完成（copy 已启动）
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // 目标与客户端相继断开：copy 出错但属直连，不应触发
        drop(client);
        let triggered =
            tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await;
        assert!(
            !matches!(triggered, Ok(Some(()))),
            "直连失败不应触发健康检查"
        );
        task.abort();
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
