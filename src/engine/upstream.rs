//! 上游拨号：协议探测、类型解析，以及经 HTTP CONNECT / SOCKS5 建立隧道。

pub mod dial_http;
pub mod dial_socks5;
#[cfg(test)]
pub(crate) mod mock;

use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::config::UpstreamKind;

use super::state::{EngineCtx, LogLevel};
use super::stream::PrefixedStream;
pub use dial_socks5::Credentials;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedKind {
    Http,
    Socks5,
}

impl ResolvedKind {
    pub fn label(self) -> &'static str {
        match self {
            ResolvedKind::Http => "http",
            ResolvedKind::Socks5 => "socks5",
        }
    }
}

/// 拨号失败的两级分类：决定路由器是否把上游标记为 DOWN。
///
/// - [`DialError::ProxyDown`]：连接层失败（拒连/超时/握手协议错误）→ 上游故障；
/// - [`DialError::OriginUnreachable`]：隧道已建立但目标不可达 → 上游仍算存活。
#[derive(Debug, Error)]
pub enum DialError {
    #[error("上游不可用: {0}")]
    ProxyDown(String),
    #[error("目标不可达: {0}")]
    OriginUnreachable(String),
}

/// 与上游建立 TCP 连接（带超时）。
pub async fn connect_timeout(
    addr: &str,
    timeout: Duration,
) -> std::io::Result<tokio::net::TcpStream> {
    match tokio::time::timeout(timeout, tokio::net::TcpStream::connect(addr)).await {
        Ok(r) => r,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "连接超时",
        )),
    }
}

/// 向上游发送 SOCKS5 握手探测：应答首字节为 0x05 判为 SOCKS5，
/// HTTP 文本应答或直接断开判为 HTTP 代理。
pub async fn probe(addr: &str, timeout: Duration) -> Result<ResolvedKind, String> {
    let fut = async {
        let mut s = connect_timeout(addr, timeout)
            .await
            .map_err(|e| format!("{addr} 连接失败: {e}"))?;
        s.write_all(&[0x05, 0x01, 0x00])
            .await
            .map_err(|e| format!("{addr} 发送探测包失败: {e}"))?;
        let mut b = [0u8; 2];
        let mut got = 0;
        while got < 2 {
            let n = s
                .read(&mut b[got..])
                .await
                .map_err(|e| format!("{addr} 读取探测应答失败: {e}"))?;
            if n == 0 {
                break;
            }
            got += n;
        }
        Ok::<_, String>(if got >= 1 && b[0] == 0x05 {
            ResolvedKind::Socks5
        } else {
            ResolvedKind::Http
        })
    };
    match tokio::time::timeout(timeout, fut).await {
        Ok(r) => r,
        Err(_) => Err(format!("{addr} 探测超时")),
    }
}

/// 解析上游的线路协议：显式指定直接返回；auto 类型探测一次后缓存。
pub async fn resolve_kind(ctx: &Arc<EngineCtx>, idx: usize) -> Result<ResolvedKind, String> {
    let up = &ctx.cfg.upstreams[idx];
    match up.kind {
        UpstreamKind::Http => Ok(ResolvedKind::Http),
        UpstreamKind::Socks5 => Ok(ResolvedKind::Socks5),
        UpstreamKind::Auto => {
            if let Some(k) = ctx.kind_cache.lock().unwrap().get(&up.addr) {
                return Ok(*k);
            }
            let t = dial_timeout(ctx);
            let k = probe(&up.addr, t).await?;
            ctx.kind_cache.lock().unwrap().insert(up.addr.clone(), k);
            ctx.state.update(idx, |u| {
                u.detected = Some(k.label().to_string());
            });
            ctx.log(
                LogLevel::Info,
                format!("上游 [{}] 协议探测结果: {}", up.name, k.label()),
            );
            Ok(k)
        }
    }
}

fn dial_timeout(ctx: &EngineCtx) -> Duration {
    Duration::from_secs(ctx.cfg.health.timeout_secs.max(1))
}

/// 从配置解析上游凭据：用户名为空视为未配置，密码缺省为空串。
fn credentials_of(ctx: &EngineCtx, idx: usize) -> Option<Credentials<'_>> {
    let up = &ctx.cfg.upstreams[idx];
    let user = up.username.as_deref().filter(|u| !u.is_empty())?;
    Some((user, up.password.as_deref().unwrap_or("")))
}

/// 建立到 `host:port` 的上游隧道（自动选择 HTTP CONNECT 或 SOCKS5）。
pub async fn dial(
    ctx: &Arc<EngineCtx>,
    idx: usize,
    host: &str,
    port: u16,
) -> Result<PrefixedStream, DialError> {
    let t = dial_timeout(ctx);
    let kind = resolve_kind(ctx, idx).await.map_err(DialError::ProxyDown)?;
    let addr = ctx.cfg.upstreams[idx].addr.clone();
    let auth = credentials_of(ctx, idx);
    match kind {
        ResolvedKind::Http => dial_http::dial(&addr, host, port, auth, t).await,
        ResolvedKind::Socks5 => dial_socks5::dial(&addr, host, port, auth, t).await,
    }
}

/// 纯 HTTP 转发路径的上游凭据头值。
///
/// `Proxy-Authorization` 逐跳生效（RFC 7235 §4.4）：仅当上游解析为 HTTP 型时
/// 才需要替它补凭据；SOCKS5 型上游走隧道，绝不能带，避免凭据泄漏给 origin。
pub async fn proxy_auth_header(ctx: &Arc<EngineCtx>, idx: usize) -> Option<String> {
    let kind = resolve_kind(ctx, idx).await.ok()?;
    if kind != ResolvedKind::Http {
        return None;
    }
    let (user, pass) = credentials_of(ctx, idx)?;
    Some(format!(
        "Basic {}",
        super::b64::encode(format!("{user}:{pass}").as_bytes())
    ))
}
