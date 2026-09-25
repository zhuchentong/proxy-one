//! 最小 HTTPS 客户端：手写 HTTP/1.1 + native-tls，支持经网关的 CONNECT 隧道。
//!
//! 自 update.rs 拆出的通用网络层，与 engine 的手写风格一致、无 C 构建依赖
//! （TLS 落到 schannel / openssl）：
//! - 直连：DNS → TCP（带连接超时）→ TLS；
//! - 经网关：对本机网关发 `CONNECT host:443` 建隧道后再 TLS，享受上游故障切换；
//! - [`get_follow`] 跟随最多 3 次 30x 重定向，按块回调下载进度。

use anyhow::{Context, Result, anyhow, bail};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const RW_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BODY: usize = 64 * 1024 * 1024;

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn parse_https_url(url: &str) -> Result<(String, String)> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| anyhow!("仅支持 https URL：{url}"))?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    if host.is_empty() {
        bail!("URL 缺少主机：{url}");
    }
    Ok((host.to_string(), format!("/{path}")))
}

/// 经网关建立 CONNECT 隧道（下载请求走自身端口，与验证探针同款握手）
fn connect_via_gateway(gateway: &str, host: &str) -> Result<TcpStream> {
    let mut s = TcpStream::connect(gateway).with_context(|| format!("连接网关 {gateway} 失败"))?;
    s.set_read_timeout(Some(RW_TIMEOUT))?;
    s.set_write_timeout(Some(RW_TIMEOUT))?;
    let req = format!("CONNECT {host}:443 HTTP/1.1\r\nHost: {host}:443\r\n\r\n");
    s.write_all(req.as_bytes())?;
    let mut head = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = s.read(&mut buf)?;
        if n == 0 {
            bail!("网关在 CONNECT 阶段关闭了连接");
        }
        head.extend_from_slice(&buf[..n]);
        if let Some(p) = find(&head, b"\r\n\r\n") {
            let line = String::from_utf8_lossy(&head[..p])
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
            if !line.contains(" 200") {
                bail!("网关拒绝 CONNECT：{line}");
            }
            if head.len() > p + 4 {
                bail!("隧道内出现意外数据");
            }
            return Ok(s);
        }
        if head.len() > 16 * 1024 {
            bail!("CONNECT 响应头过大");
        }
    }
}

fn connect_direct(host: &str) -> Result<TcpStream> {
    let addr = (host, 443u16)
        .to_socket_addrs()
        .with_context(|| format!("DNS 解析失败：{host}"))?
        .next()
        .ok_or_else(|| anyhow!("DNS 解析为空：{host}"))?;
    let s = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .with_context(|| format!("连接 {host} 失败"))?;
    s.set_read_timeout(Some(RW_TIMEOUT))?;
    s.set_write_timeout(Some(RW_TIMEOUT))?;
    Ok(s)
}

/// HTTP 响应：(状态码, 头部键值对（键已小写）, 响应体)
pub(crate) type Response = (u32, Vec<(String, String)>, Vec<u8>);

/// 最小 HTTPS GET：跟随最多 3 次 30x；progress 按读块刷新（每跳开始时清零）
pub(crate) fn get_follow(
    gateway: Option<&str>,
    url: &str,
    extra_headers: &str,
    progress: Option<&AtomicU64>,
) -> Result<Response> {
    let mut url = url.to_string();
    for _ in 0..3 {
        let (host, path) = parse_https_url(&url)?;
        let tcp = match gateway {
            Some(g) => connect_via_gateway(g, &host)?,
            None => connect_direct(&host)?,
        };
        let conn = native_tls::TlsConnector::builder()
            .build()
            .context("构建 TLS 连接器失败")?;
        let mut tls = conn
            .connect(&host, tcp)
            .with_context(|| format!("TLS 握手失败：{host}"))?;
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: proxyone/{}\r\n{extra_headers}Connection: close\r\n\r\n",
            env!("CARGO_PKG_VERSION")
        );
        tls.write_all(req.as_bytes())?;
        if let Some(p) = progress {
            p.store(0, Ordering::Relaxed);
        }
        let mut resp = Vec::new();
        let mut buf = [0u8; 64 * 1024];
        loop {
            match tls.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    resp.extend_from_slice(&buf[..n]);
                    if let Some(p) = progress {
                        p.fetch_add(n as u64, Ordering::Relaxed);
                    }
                }
                Err(e) => return Err(anyhow!(e)).context("读取响应失败"),
            }
            if resp.len() > MAX_BODY {
                bail!("响应超过大小上限");
            }
        }
        drop(tls);
        let hp = find(&resp, b"\r\n\r\n").ok_or_else(|| anyhow!("响应缺少头部结束标记"))?;
        let head = String::from_utf8_lossy(&resp[..hp]);
        let status: u32 = head
            .lines()
            .next()
            .unwrap_or("")
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let headers: Vec<(String, String)> = head
            .lines()
            .skip(1)
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
            .collect();
        if (300..400).contains(&status) {
            url = headers
                .iter()
                .find(|(k, _)| k == "location")
                .map(|(_, v)| v.clone())
                .ok_or_else(|| anyhow!("重定向缺少 Location"))?;
            continue;
        }
        return Ok((status, headers, resp[hp + 4..].to_vec()));
    }
    bail!("重定向次数过多")
}

#[cfg(test)]
mod tests {
    use super::parse_https_url;

    #[test]
    fn https_url_splits_host_and_path() {
        let (host, path) =
            parse_https_url("https://api.github.com/repos/a/b/releases/latest").unwrap();
        assert_eq!(host, "api.github.com");
        assert_eq!(path, "/repos/a/b/releases/latest");

        // 无路径 → 根路径；多余端口/凭证保留在 host 中交由连接层处理
        let (host, path) = parse_https_url("https://example.com").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(path, "/");

        assert!(parse_https_url("http://example.com/").is_err());
        assert!(parse_https_url("https:///path-only").is_err());
    }
}
