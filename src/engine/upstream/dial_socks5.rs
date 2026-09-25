//! 经 SOCKS5 上游建立 CONNECT 隧道。
//!
//! 认证（RFC 1929）：配置了用户名/密码时，greeting 同时提供
//! `用户名密码(0x02)`（优先）与 `无认证(0x00)`（兜底）两种方法，
//! 服务器选定 0x02 才进入子协商；域名以 ATYP=domain 透传给上游解析。

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::super::stream::PrefixedStream;
use super::{DialError, connect_timeout};

/// 上游凭据：`(用户名, 密码)`。
pub type Credentials<'a> = (&'a str, &'a str);

/// 方法协商 greeting：有凭据时 0x02 优先 + 0x00 兜底（curl 风格），
/// 无凭据时仅提供 0x00。
pub(crate) fn greeting(with_auth: bool) -> Vec<u8> {
    if with_auth {
        vec![0x05, 0x02, 0x02, 0x00]
    } else {
        vec![0x05, 0x01, 0x00]
    }
}

/// 构造 RFC 1929 用户名/密码子协商请求；长度超限在上游必然被拒，提前报错。
pub(crate) fn auth_request(user: &str, pass: &str) -> Result<Vec<u8>, DialError> {
    if user.len() > 255 || pass.len() > 255 {
        return Err(DialError::ProxyDown(
            "用户名/密码长度超过 SOCKS5 上限（255 字节）".into(),
        ));
    }
    let mut v = Vec::with_capacity(3 + user.len() + pass.len());
    v.push(0x01); // 子协商版本
    v.push(user.len() as u8);
    v.extend_from_slice(user.as_bytes());
    v.push(pass.len() as u8);
    v.extend_from_slice(pass.as_bytes());
    Ok(v)
}

/// 完成握手（含按需认证）并发送 CONNECT，应答码 0x00 即隧道建立。
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
        s.write_all(&greeting(auth.is_some()))
            .await
            .map_err(|e| DialError::ProxyDown(format!("发送握手失败: {e}")))?;
        let mut hello = [0u8; 2];
        s.read_exact(&mut hello)
            .await
            .map_err(|e| DialError::ProxyDown(format!("读取握手应答失败: {e}")))?;
        if hello[0] != 0x05 {
            return Err(DialError::ProxyDown(format!(
                "非 SOCKS5 应答: {:02x} {:02x}",
                hello[0], hello[1]
            )));
        }
        match hello[1] {
            0x00 => {} // 无认证
            0x02 => {
                let Some((user, pass)) = auth else {
                    return Err(DialError::ProxyDown(
                        "上游要求用户名/密码认证，但未配置凭据".into(),
                    ));
                };
                let req = auth_request(user, pass)?;
                s.write_all(&req)
                    .await
                    .map_err(|e| DialError::ProxyDown(format!("发送认证失败: {e}")))?;
                let mut ack = [0u8; 2];
                s.read_exact(&mut ack)
                    .await
                    .map_err(|e| DialError::ProxyDown(format!("读取认证应答失败: {e}")))?;
                if ack[0] != 0x01 {
                    return Err(DialError::ProxyDown(format!(
                        "认证应答版本异常: {:02x}",
                        ack[0]
                    )));
                }
                if ack[1] != 0x00 {
                    return Err(DialError::ProxyDown("用户名/密码认证被上游拒绝".into()));
                }
            }
            0xFF => {
                return Err(DialError::ProxyDown("上游不接受任何提供的认证方式".into()));
            }
            other => {
                return Err(DialError::ProxyDown(format!(
                    "上游选择了不支持的认证方式 {other:#04x}"
                )));
            }
        }
        if host.len() > 255 {
            return Err(DialError::OriginUnreachable("域名过长".into()));
        }

        let mut req = vec![0x05, 0x01, 0x00];
        match host.parse::<std::net::Ipv4Addr>() {
            Ok(ip4) => {
                req.push(0x01);
                req.extend_from_slice(&ip4.octets());
            }
            Err(_) => match host.parse::<std::net::Ipv6Addr>() {
                Ok(ip6) => {
                    req.push(0x04);
                    req.extend_from_slice(&ip6.octets());
                }
                Err(_) => {
                    req.push(0x03);
                    req.push(host.len() as u8);
                    req.extend_from_slice(host.as_bytes());
                }
            },
        }
        req.extend_from_slice(&port.to_be_bytes());
        s.write_all(&req)
            .await
            .map_err(|e| DialError::ProxyDown(format!("发送 CONNECT 请求失败: {e}")))?;

        let mut head = [0u8; 4];
        s.read_exact(&mut head)
            .await
            .map_err(|e| DialError::ProxyDown(format!("读取 CONNECT 应答失败: {e}")))?;
        if head[0] != 0x05 {
            return Err(DialError::ProxyDown("SOCKS5 应答版本异常".into()));
        }
        if head[1] != 0x00 {
            return Err(DialError::OriginUnreachable(format!(
                "SOCKS5 错误码 {:#04x}: {}",
                head[1],
                error_message(head[1])
            )));
        }
        let skip = match head[3] {
            0x01 => 4,
            0x04 => 16,
            0x03 => {
                let mut l = [0u8; 1];
                s.read_exact(&mut l)
                    .await
                    .map_err(|e| DialError::ProxyDown(format!("读取应答失败: {e}")))?;
                l[0] as usize
            }
            _ => {
                return Err(DialError::ProxyDown(format!(
                    "未知地址类型 {:#04x}",
                    head[3]
                )));
            }
        };
        let mut rest = vec![0u8; skip + 2];
        s.read_exact(&mut rest)
            .await
            .map_err(|e| DialError::ProxyDown(format!("读取应答尾部失败: {e}")))?;
        Ok(PrefixedStream::new(s, Vec::new()))
    };
    match tokio::time::timeout(timeout * 2, fut).await {
        Ok(r) => r,
        Err(_) => Err(DialError::ProxyDown("SOCKS5 握手超时".into())),
    }
}

fn error_message(code: u8) -> &'static str {
    match code {
        0x01 => "一般性失败",
        0x02 => "规则不允许",
        0x03 => "网络不可达",
        0x04 => "主机不可达",
        0x05 => "连接被拒绝",
        0x06 => "TTL 过期",
        0x07 => "命令不支持",
        0x08 => "地址类型不支持",
        _ => "未知错误",
    }
}

#[cfg(test)]
mod tests {
    use super::{auth_request, dial, greeting};
    use std::time::Duration;

    #[test]
    fn greeting_offers_auth_first_when_configured() {
        assert_eq!(greeting(false), vec![0x05, 0x01, 0x00]);
        assert_eq!(greeting(true), vec![0x05, 0x02, 0x02, 0x00]);
    }

    #[test]
    fn auth_request_matches_rfc1929_layout() {
        let req = auth_request("user", "pass").unwrap();
        assert_eq!(
            req,
            vec![0x01, 4, b'u', b's', b'e', b'r', 4, b'p', b'a', b's', b's']
        );
    }

    #[test]
    fn auth_request_rejects_overlong_fields() {
        let long = "x".repeat(256);
        assert!(auth_request(&long, "p").is_err());
        assert!(auth_request("u", &long).is_err());
    }

    #[tokio::test]
    async fn authenticates_with_username_password() {
        let addr = super::super::mock::socks5_proxy("user", "pass").await;
        let r = dial(
            &addr,
            "example.com",
            80,
            Some(("user", "pass")),
            Duration::from_secs(2),
        )
        .await;
        assert!(r.is_ok(), "正确凭据应能完成认证并建立隧道: {:?}", r.err());
    }

    #[tokio::test]
    async fn rejected_when_only_no_auth_offered() {
        let addr = super::super::mock::socks5_proxy("user", "pass").await;
        let err = dial(&addr, "example.com", 80, None, Duration::from_secs(2))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("不接受"), "got: {err}");
    }
}
