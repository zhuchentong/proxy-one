//! 代理各处共用的 URL / authority 解析工具。

/// 从 `host[:port][/path][?query]` 形式的字符串里拆出 authority 与 path。
/// 常见输入如 `www.gstatic.com/generate_204`、`example.com?x=1`。
pub fn authority_path(rest: &str) -> Option<(&str, String)> {
    // 忽略 userinfo（user:pass@host 中的前半段）
    let rest = rest.rsplit('@').next().unwrap_or(rest);
    match rest.find(['/', '?']) {
        Some(i) => {
            let tail = &rest[i..];
            // 仅带 query 时补上根路径，保证 origin-form 以 / 开头
            let path = if tail.starts_with('/') {
                tail.to_string()
            } else {
                format!("/{tail}")
            };
            Some((&rest[..i], path))
        }
        None => Some((rest, "/".to_string())),
    }
}

/// 拆分 authority 为 (host, port)，支持 `[v6]:port`、`host:port`、`host` 三种形态。
pub fn split_host_port(authority: &str, default_port: u16) -> Option<(String, u16)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']')?;
        let host = format!("[{}]", &rest[..end]);
        let after = &rest[end + 1..];
        let port = after
            .strip_prefix(':')
            .and_then(|p| p.parse().ok())
            .unwrap_or(default_port);
        return Some((host, port));
    }
    match authority.rfind(':') {
        Some(p) => {
            let h = &authority[..p];
            let port = authority[p + 1..].parse().ok()?;
            Some((h.to_string(), port))
        }
        None => Some((authority.to_string(), default_port)),
    }
}

/// 解析健康检查目标（仅支持 http:// 明文探测）→ (host, port, path)。
pub fn parse_http_target(url: &str) -> Result<(String, u16, String), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("test_url 仅支持 http://（当前: {url}）"))?;
    let (auth, path) = authority_path(rest).ok_or_else(|| "test_url 解析失败".to_string())?;
    let (h, p) = split_host_port(auth, 80).ok_or_else(|| "test_url 主机解析失败".to_string())?;
    Ok((h, p, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_host_port_handles_common_forms() {
        assert_eq!(
            split_host_port("example.com", 80).unwrap(),
            ("example.com".into(), 80)
        );
        assert_eq!(
            split_host_port("example.com:8080", 80).unwrap(),
            ("example.com".into(), 8080)
        );
        assert_eq!(
            split_host_port("[2001:db8::1]:443", 80).unwrap(),
            ("[2001:db8::1]".into(), 443)
        );
        assert_eq!(
            split_host_port("[2001:db8::1]", 443).unwrap(),
            ("[2001:db8::1]".into(), 443)
        );
        assert!(split_host_port("host:not-a-port", 80).is_none());
    }

    #[test]
    fn authority_path_splits_and_strips_userinfo() {
        assert_eq!(
            authority_path("example.com/generate_204").unwrap(),
            ("example.com", "/generate_204".to_string())
        );
        assert_eq!(
            authority_path("user:pass@example.com?a=1").unwrap(),
            ("example.com", "/?a=1".to_string())
        );
        assert_eq!(
            authority_path("example.com").unwrap(),
            ("example.com", "/".to_string())
        );
    }

    #[test]
    fn parse_http_target_accepts_only_http() {
        let (h, p, path) = parse_http_target("http://www.gstatic.com/generate_204").unwrap();
        assert_eq!(
            (h.as_str(), p, path.as_str()),
            ("www.gstatic.com", 80, "/generate_204")
        );
        assert!(parse_http_target("https://www.gstatic.com/generate_204").is_err());
    }
}
