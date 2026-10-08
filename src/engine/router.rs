//! 上游路由：按优先级选择存活上游，并在转发任何字节之前完成就地故障切换。

use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;

use super::state::{EngineCtx, HealthStatus, LogLevel};
use super::stream::PrefixedStream;
use super::upstream::{self, DialError};

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("直连失败: {0}")]
    Direct(String),
    #[error("所有上游均连接失败: {0}")]
    Exhausted(String),
}

/// 纯函数：给定按配置顺序排列的优先级与状态，产出本连接的候选尝试顺序。
///
/// 规则：优先级数字小者优先（同优先级按配置顺序）；
/// 状态为 DOWN 的候选仅在全部 DOWN 时才参与尝试（尽力拨号）。
pub fn candidate_order(priorities: &[i32], statuses: &[HealthStatus]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..priorities.len()).collect();
    order.sort_by_key(|&i| (priorities[i], i));
    let eligible: Vec<usize> = order
        .iter()
        .copied()
        .filter(|&i| statuses.get(i).copied() != Some(HealthStatus::Down))
        .collect();
    if eligible.is_empty() { order } else { eligible }
}

/// 排除名单匹配：命中则该目标直连、不经上游。
///
/// host 与条目均小写化比较，条目两端空白忽略；`*` 只在两种位置有特殊含义：
/// - `*.example.com`：仅子域、任意深度，不含裸域（裸域请另写一条 `example.com`）；
/// - `192.168.*`：按前缀通配（裸 `*` 即匹配一切）；
/// - 其余条目：域名后缀语义（该域名及其任意子域）或 IP 字面量精确匹配。
pub fn is_bypassed(host: &str, rules: &[String]) -> bool {
    let host = normalize_host(host);
    if host.is_empty() {
        return false;
    }
    rules.iter().any(|rule| {
        let rule = normalize_host(rule);
        if rule.is_empty() {
            return false;
        }
        if let Some(domain) = rule.strip_prefix("*.") {
            subdomain_of(&host, domain).is_some()
        } else if let Some(prefix) = rule.strip_suffix('*') {
            host.starts_with(prefix)
        } else {
            host == rule || subdomain_of(&host, &rule).is_some()
        }
    })
}

/// 小写化、去 FQDN 尾点、去 IPv6 方括号。
fn normalize_host(s: &str) -> String {
    let s = s.trim().to_ascii_lowercase();
    let s = s.strip_suffix('.').unwrap_or(&s);
    s.trim_start_matches('[')
        .trim_end_matches(']')
        .to_string()
}

/// `host` 是 `domain` 的子域时返回剩余前缀（以 `.` 结尾），否则 `None`。
fn subdomain_of<'a>(host: &'a str, domain: &str) -> Option<&'a str> {
    host.strip_suffix(domain).filter(|s| s.ends_with('.'))
}

/// 直连拨号：排除名单命中与零上游回退共用，失败不影响上游健康状态。
async fn dial_direct(
    ctx: &Arc<EngineCtx>,
    proto: &str,
    host: &str,
    port: u16,
) -> Result<(Route, PrefixedStream), ConnectError> {
    let timeout = Duration::from_secs(ctx.cfg.health.timeout_secs);
    match upstream::connect_timeout(&format!("{host}:{port}"), timeout).await {
        Ok(s) => {
            if ctx.cfg.general.forward_log {
                ctx.log(LogLevel::Info, format!("→ {proto} {host}:{port} 直连"));
            }
            Ok((Route::Direct, PrefixedStream::new(s, Vec::new())))
        }
        Err(e) => Err(ConnectError::Direct(format!(
            "直连 {host}:{port} 失败: {e}"
        ))),
    }
}

/// 本连接的出站路径：排除名单命中走 [`Route::Direct`]，否则经上游。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// 直连目标：不经任何上游，也不入上游流量统计。
    Direct,
    /// 经配置中第 `usize` 个上游隧道转发。
    Upstream(usize),
}

/// 为目标 `host:port` 选择一条出站路径并建立连接。
///
/// `proto` 是入站协议标签（CONNECT / GET / SOCKS5…），仅用于转发日志。
///
/// 排除名单命中或未配置任何上游时直接拨号目标（对客户端透明，失败报
/// [`ConnectError::Direct`] 且不影响上游健康状态）；否则按优先级选存活上游，
/// 并在转发任何字节之前完成就地故障切换：连接层失败会立即把该上游标记
/// DOWN 并触发一轮健康检查，然后继续尝试下一个候选。
pub async fn connect_target(
    ctx: &Arc<EngineCtx>,
    proto: &str,
    host: &str,
    port: u16,
) -> Result<(Route, PrefixedStream), ConnectError> {
    if is_bypassed(host, &ctx.cfg.general.bypass_hosts) || ctx.cfg.upstreams.is_empty() {
        return dial_direct(ctx, proto, host, port).await;
    }
    let priorities: Vec<i32> = ctx.cfg.upstreams.iter().map(|u| u.priority).collect();
    let statuses = ctx.state.statuses();
    let candidates = candidate_order(&priorities, &statuses);

    let mut last_err = String::from("无候选上游");
    for i in candidates {
        let name = ctx.cfg.upstreams[i].name.clone();
        match upstream::dial(ctx, i, host, port).await {
            Ok(s) => {
                ctx.state.record_conn(i);
                if ctx.state.set_active(&name) {
                    ctx.log(
                        LogLevel::Ok,
                        format!("🔀 使用上游 [{name}] 服务 {host}:{port}"),
                    );
                }
                if ctx.cfg.general.forward_log {
                    ctx.log(
                        LogLevel::Info,
                        format!("→ {proto} {host}:{port} 经 [{name}]"),
                    );
                }
                return Ok((Route::Upstream(i), s));
            }
            Err(DialError::ProxyDown(msg)) => {
                let changed = ctx.mark_down(i);
                if changed {
                    ctx.log(
                        LogLevel::Error,
                        format!("⛔ 上游 [{name}] 拨号失败 → 标记 DOWN: {msg}"),
                    );
                } else {
                    ctx.log(
                        LogLevel::Warn,
                        format!("上游 [{name}] 拨号失败（已 DOWN）: {msg}"),
                    );
                }
                let _ = ctx.health_tx.send(());
                last_err = format!("[{name}] {msg}");
            }
            Err(DialError::OriginUnreachable(msg)) => {
                ctx.log(
                    LogLevel::Warn,
                    format!("经上游 [{name}] 连接 {host}:{port} 失败: {msg}"),
                );
                last_err = format!("[{name}] {msg}");
            }
        }
    }
    Err(ConnectError::Exhausted(last_err))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::Config;
    use crate::engine::state::StateStore;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;

    const UP: HealthStatus = HealthStatus::Up;
    const DOWN: HealthStatus = HealthStatus::Down;
    const UNKNOWN: HealthStatus = HealthStatus::Unknown;

    fn statuses(values: &[HealthStatus]) -> Vec<HealthStatus> {
        values.to_vec()
    }

    /// 测试用引擎上下文：真实 Config/StateStore，超时压到 1s。
    fn ctx_with(rules: &[&str], upstreams: bool) -> Arc<EngineCtx> {
        let mut cfg = Config::default();
        if !upstreams {
            cfg.upstreams.clear();
        }
        cfg.general.bypass_hosts = rules.iter().map(|s| s.to_string()).collect();
        cfg.health.timeout_secs = 1;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        Arc::new(EngineCtx {
            cfg: Arc::new(cfg.clone()),
            state: Arc::new(StateStore::new(&cfg)),
            health_tx: tx,
            kind_cache: Mutex::new(HashMap::new()),
            url_warned: AtomicBool::new(false),
        })
    }

    #[test]
    fn orders_by_priority_then_config_index() {
        let pri = [2, 1, 1];
        let st = statuses(&[UP, UP, UP]);
        assert_eq!(candidate_order(&pri, &st), vec![1, 2, 0]);
    }

    #[test]
    fn skips_down_but_keeps_priority_order() {
        let pri = [1, 2, 3];
        let st = statuses(&[DOWN, UP, UP]);
        assert_eq!(candidate_order(&pri, &st), vec![1, 2]);
    }

    #[test]
    fn all_down_falls_back_to_all() {
        let pri = [1, 2];
        let st = statuses(&[DOWN, DOWN]);
        assert_eq!(candidate_order(&pri, &st), vec![0, 1]);
    }

    #[test]
    fn unknown_is_eligible() {
        let pri = [1, 2];
        let st = statuses(&[UNKNOWN, UNKNOWN]);
        assert_eq!(candidate_order(&pri, &st), vec![0, 1]);
    }

    fn one(rule: &str) -> Vec<String> {
        vec![rule.to_string()]
    }

    #[test]
    fn bypass_suffix_rule_covers_domain_and_subdomains() {
        let r = one("example.com");
        assert!(is_bypassed("example.com", &r));
        assert!(is_bypassed("sub.example.com", &r));
        assert!(is_bypassed("deep.sub.example.com", &r));
        assert!(!is_bypassed("notexample.com", &r));
        assert!(!is_bypassed("example.org", &r));
    }

    #[test]
    fn bypass_star_dot_rule_excludes_bare_domain() {
        let r = one("*.example.com");
        assert!(is_bypassed("sub.example.com", &r));
        assert!(is_bypassed("a.b.example.com", &r));
        assert!(!is_bypassed("example.com", &r));
        assert!(!is_bypassed("aexample.com", &r));
    }

    #[test]
    fn bypass_trailing_star_prefixes_ip() {
        let r = one("192.168.*");
        assert!(is_bypassed("192.168.1.1", &r));
        assert!(is_bypassed("192.168.0.0", &r));
        assert!(!is_bypassed("10.0.0.1", &r));
        assert!(!is_bypassed("192.169.1.1", &r));
    }

    #[test]
    fn bypass_matches_ip_literals() {
        assert!(is_bypassed("127.0.0.1", &one("127.0.0.1")));
        assert!(is_bypassed("::1", &one("::1")));
        assert!(!is_bypassed("::2", &one("::1")));
    }

    #[test]
    fn bypass_is_case_and_bracket_tolerant() {
        let r = one("Example.COM");
        assert!(is_bypassed("SUB.example.com", &r));
        assert!(is_bypassed("::1", &one("[::1]")));
        assert!(is_bypassed("[::1]", &one("::1")));
        assert!(is_bypassed("example.com.", &one("example.com")));
    }

    #[test]
    fn bypass_blank_rules_and_hosts_never_match() {
        assert!(!is_bypassed("example.com", &[]));
        assert!(!is_bypassed("example.com", &one("   ")));
        assert!(!is_bypassed("  ", &one("example.com")));
    }

    #[test]
    fn bypass_bare_star_matches_everything() {
        assert!(is_bypassed("anything.internal", &one("*")));
        assert!(is_bypassed("1.2.3.4", &one("*")));
    }

    #[tokio::test]
    async fn bypassed_host_dials_direct_even_without_upstreams() {
        // 名单命中 + 零上游：直连独立于上游配置，必须成功且不报 NoUpstreams
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ctx = ctx_with(&["127.*"], false);

        let (route, _up) =
            connect_target(&ctx, "CONNECT", &addr.ip().to_string(), addr.port())
                .await
                .unwrap();
        assert_eq!(route, Route::Direct);

        // 直连是真实 TCP 连接：listener 侧能 accept 到
        let (_sock, peer) = listener.accept().await.unwrap();
        assert_eq!(peer.ip().to_string(), "127.0.0.1");
    }

    #[tokio::test]
    async fn direct_failure_yields_direct_error_and_spares_upstreams() {
        // 名单命中但目标拒连：直连失败报 Direct，且上游健康状态不受牵连
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let ctx = ctx_with(&["127.*"], true);

        let err = connect_target(&ctx, "CONNECT", &addr.ip().to_string(), addr.port())
            .await
            .unwrap_err();
        assert!(matches!(err, ConnectError::Direct(_)));
        assert!(ctx.state.statuses().iter().all(|s| *s == UNKNOWN));
    }

    #[tokio::test]
    async fn zero_upstreams_falls_back_to_direct() {
        // 未配置上游：未命中名单的请求也转为直连，而非直接失败
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ctx = ctx_with(&[], false);

        let (route, _up) =
            connect_target(&ctx, "CONNECT", &addr.ip().to_string(), addr.port())
                .await
                .unwrap();
        assert_eq!(route, Route::Direct);
        let (_sock, _peer) = listener.accept().await.unwrap();
    }
}
