//! 上游路由：按优先级选择存活上游，并在转发任何字节之前完成就地故障切换。

use std::sync::Arc;

use thiserror::Error;

use super::state::{EngineCtx, HealthStatus, LogLevel};
use super::stream::PrefixedStream;
use super::upstream::{self, DialError};

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("没有配置任何上游")]
    NoUpstreams,
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

/// 为目标 `host:port` 选择一个上游并建立隧道。
///
/// `proto` 是入站协议标签（CONNECT / GET / SOCKS5…），仅用于转发日志。
///
/// 失败语义见 [`DialError`]：连接层失败会立即把该上游标记 DOWN 并触发一轮
/// 健康检查，然后继续尝试下一个候选；这一切都发生在向客户端转发任何
/// 字节之前，对客户端透明。
pub async fn connect_target(
    ctx: &Arc<EngineCtx>,
    proto: &str,
    host: &str,
    port: u16,
) -> Result<(usize, PrefixedStream), ConnectError> {
    if ctx.cfg.upstreams.is_empty() {
        return Err(ConnectError::NoUpstreams);
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
                return Ok((i, s));
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

    const UP: HealthStatus = HealthStatus::Up;
    const DOWN: HealthStatus = HealthStatus::Down;
    const UNKNOWN: HealthStatus = HealthStatus::Unknown;

    fn statuses(values: &[HealthStatus]) -> Vec<HealthStatus> {
        values.to_vec()
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
}
