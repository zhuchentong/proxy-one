//! 经 HTTP 代理上游建立 CONNECT 隧道。
//!
//! 认证：配置了用户名/密码时在 CONNECT 请求上追加
//! `Proxy-Authorization: Basic base64(user:pass)`（RFC 7235）。

use std::time::Duration;

use tokio::io::AsyncWriteExt;

use super::super::b64;
use super::super::stream::{PrefixedStream, read_head};
use super::{Credentials, DialError, connect_timeout};

/// 对上游发送 `CONNECT host:port`，应答 2xx 即隧道建立。
pub async fn dial(
    addr: &str,
    host: &str,
    port: u16,
    auth: Option<Credentials<'_>>,
    timeout: Duration,
) -> Result<PrefixedStream, DialError> {
    let fut = async {
        let mut s = connect_timeout(addr, timeout)
            .await
            .map_err(|e| DialError::ProxyDown(format!("{addr} 连接失败: {e}")))?;
        let mut req = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n");
        if let Some((user, pass)) = &auth {
            let token = b64::encode(format!("{user}:{pass}").as_bytes());
            req.push_str(&format!("Proxy-Authorization: Basic {token}\r\n"));
        }
        req.push_str("\r\n");
        s.write_all(req.as_bytes())
            .await
            .map_err(|e| DialError::ProxyDown(format!("发送 CONNECT 失败: {e}")))?;
        let (head, leftover) = read_head(&mut s, 16 * 1024)
            .await
            .map_err(|e| DialError::ProxyDown(format!("读取 CONNECT 应答失败: {e}")))?;
        let line = String::from_utf8_lossy(head.split(|&b| b == b'\n').next().unwrap_or(&[]));
        let code = line
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse::<u16>().ok());
        match code {
            Some(c) if (200..300).contains(&c) => Ok(PrefixedStream::new(s, leftover)),
            Some(407) => Err(DialError::ProxyDown(
                "上游要求代理认证（407）：请检查配置中的用户名/密码".into(),
            )),
            Some(c) => Err(DialError::OriginUnreachable(format!("CONNECT 返回 {c}"))),
            None => Err(DialError::ProxyDown(format!(
                "CONNECT 应答异常: {}",
                line.trim()
            ))),
        }
    };
    match tokio::time::timeout(timeout * 2, fut).await {
        Ok(r) => r,
        Err(_) => Err(DialError::ProxyDown("CONNECT 握手超时".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// base64("user:pass")
    const TOKEN: &str = "dXNlcjpwYXNz";

    #[tokio::test]
    async fn sends_basic_credentials_to_http_upstream() {
        let addr = super::super::mock::http_proxy(TOKEN).await;
        let r = dial(
            &addr,
            "example.com",
            80,
            Some(("user", "pass")),
            Duration::from_secs(2),
        )
        .await;
        assert!(r.is_ok(), "带正确凭据应能建立隧道: {:?}", r.err());
    }

    #[tokio::test]
    async fn wrong_credentials_map_407_to_proxy_down() {
        let addr = super::super::mock::http_proxy(TOKEN).await;
        let err = dial(
            &addr,
            "example.com",
            80,
            Some(("user", "bad")),
            Duration::from_secs(2),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("407"), "got: {err}");
    }
}
