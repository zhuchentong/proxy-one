//! 主动健康检查：经每个上游对测试地址发一次 HTTP 请求，按滞后阈值切换 UP/DOWN。

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, watch};

use super::state::{EngineCtx, HealthStatus, LogLevel};
use super::upstream;
use super::url::parse_http_target;

pub async fn run(
    ctx: Arc<EngineCtx>,
    mut trigger: mpsc::UnboundedReceiver<()>,
    mut manual_rx: mpsc::UnboundedReceiver<()>,
    mut stop: watch::Receiver<bool>,
) {
    let interval = Duration::from_secs(ctx.cfg.health.interval_secs.max(1));
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await; // 消费立即触发的第一次 tick
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = tick.tick() => {}
            _ = trigger.recv() => {
                while trigger.try_recv().is_ok() {} // 去抖：一轮期间的多次触发合并
            }
            msg = manual_rx.recv() => match msg {
                // 手动测试全部上游：并发执行，不打断周期性检查
                Some(()) => {
                    let ctx = ctx.clone();
                    tokio::spawn(handle_manual_all(ctx));
                    continue;
                }
                None => break,
            }
        }
        if *stop.borrow() {
            break;
        }
        round(&ctx, false).await;
    }
    ctx.log(LogLevel::Info, "健康检查循环已退出");
}

async fn handle_manual_all(ctx: Arc<EngineCtx>) {
    let n = ctx.cfg.upstreams.len();
    if n == 0 {
        ctx.log(LogLevel::Warn, "没有可测试的上游");
        return;
    }
    ctx.log(
        LogLevel::Info,
        format!("🔍 开始手动测试全部上游（{n} 个）…"),
    );
    for i in 0..n {
        ctx.state.begin_testing(i);
    }
    round(&ctx, true).await;
    for i in 0..n {
        ctx.state.end_testing(i);
    }
}

async fn round(ctx: &Arc<EngineCtx>, manual: bool) {
    let n = ctx.cfg.upstreams.len();
    if n == 0 {
        return;
    }
    let mut handles = Vec::with_capacity(n);
    for i in 0..n {
        handles.push(tokio::spawn(check_one(ctx.clone(), i, manual)));
    }
    for h in handles {
        let _ = h.await;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Transition {
    None,
    ToUp,
    ToDown,
}

/// 纯函数：一次探测结果对上游状态的滞后更新。
///
/// 连续成功 `thr_ok` 次才恢复 UP，连续失败 `thr_fail` 次才标记 DOWN，
/// 避免状态在阈值附近来回抖动。
pub(crate) fn transition(
    ok: bool,
    ok_streak: &mut u32,
    fail_streak: &mut u32,
    thr_ok: u32,
    thr_fail: u32,
    status: &mut HealthStatus,
) -> Transition {
    if ok {
        *ok_streak += 1;
        *fail_streak = 0;
        if *ok_streak >= thr_ok && *status != HealthStatus::Up {
            *status = HealthStatus::Up;
            return Transition::ToUp;
        }
    } else {
        *fail_streak += 1;
        *ok_streak = 0;
        if *fail_streak >= thr_fail && *status != HealthStatus::Down {
            *status = HealthStatus::Down;
            return Transition::ToDown;
        }
    }
    Transition::None
}

/// 周期性检查失败时的日志策略（仅用于探测失败，成功路径不经过此函数）。
///
/// - 转 DOWN → [`LogLevel::Error`]（手动测试也保留，与既有行为一致）；
/// - UP 下连续失败第 1 次 → [`LogLevel::Warn`]：每个失败段开头提示一次。
///   注：`fail_threshold = 1` 时首次失败即转 DOWN，本分支不可达，属预期；
/// - DOWN 后的重复失败 → 心跳节流：每逢 `fail_streak` 是 `warn_every` 的
///   整数倍打一条 [`LogLevel::Warn`]（调用方按检查间隔折算，约每分钟一条），
///   其余静默：既保留长 DOWN 的可见性，又不刷屏；
/// - [`HealthStatus::Unknown`]（启动后尚未成功过）期间首败 → `None` 静默：
///   启动即执行一轮检查，距 ERR DOWN 至多一轮，静默无害。
fn failure_log_level(
    t: Transition,
    prev: HealthStatus,
    fail_streak: u32,
    warn_every: u32,
) -> Option<LogLevel> {
    match (t, prev, fail_streak) {
        (Transition::ToDown, _, _) => Some(LogLevel::Error),
        (Transition::None, HealthStatus::Up, 1) => Some(LogLevel::Warn),
        (Transition::None, HealthStatus::Down, n) => {
            (n > 0 && n % warn_every.max(1) == 0).then_some(LogLevel::Warn)
        }
        _ => None,
    }
}

async fn check_one(ctx: Arc<EngineCtx>, idx: usize, manual: bool) {
    let target = match parse_http_target(&ctx.cfg.health.test_url) {
        Ok(t) => t,
        Err(e) => {
            if !ctx
                .url_warned
                .swap(true, std::sync::atomic::Ordering::Relaxed)
            {
                ctx.log(
                    LogLevel::Error,
                    format!(
                        "test_url 无效（{}），健康检查已跳过: {e}",
                        ctx.cfg.health.test_url
                    ),
                );
            }
            return;
        }
    };
    let (host, port, path) = target;
    let name = ctx.cfg.upstreams[idx].name.clone();
    let timeout = Duration::from_secs(ctx.cfg.health.timeout_secs.max(1) * 2 + 2);
    let started = Instant::now();

    let fut = async {
        let mut s = upstream::dial(&ctx, idx, &host, port)
            .await
            .map_err(|e| e.to_string())?;
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: proxyone-health\r\nConnection: close\r\n\r\n"
        );
        s.write_all(req.as_bytes())
            .await
            .map_err(|e| format!("发送探测请求失败: {e}"))?;
        let (head, _left) = super::stream::read_head(&mut s, 16 * 1024)
            .await
            .map_err(|e| format!("读取探测响应失败: {e}"))?;
        let line = String::from_utf8_lossy(head.split(|&b| b == b'\n').next().unwrap_or(&[]));
        let code = line
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse::<u16>().ok());
        match code {
            Some(c) if (200..400).contains(&c) => Ok(()),
            Some(c) => Err(format!("HTTP {c}")),
            None => Err(format!("响应行异常: {}", line.trim())),
        }
    };

    let outcome = match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("探测超时".to_string()),
    };
    ctx.state.record_check(idx, outcome.is_ok());
    let ms = started.elapsed().as_millis() as u64;
    let now = super::state::now_string();
    let thr_ok = ctx.cfg.health.success_threshold.max(1);
    let thr_fail = ctx.cfg.health.fail_threshold.max(1);

    // 手动测试总会给出一条结果日志；周期性检查仅在 UP 下首次失败（Warn 一次）、
    // 转 DOWN（Err）与 DOWN 心跳（Warn，约每分钟一条）时说话，其余失败静默避免刷屏
    if manual {
        match &outcome {
            Ok(()) => ctx.log(
                LogLevel::Info,
                format!("🔍 手动测试 [{name}] 成功：{ms} ms"),
            ),
            Err(reason) => ctx.log(
                LogLevel::Warn,
                format!("🔍 手动测试 [{name}] 失败：{reason}"),
            ),
        }
    }

    match outcome {
        Ok(()) => {
            let recovered = ctx.state.update(idx, |u| {
                u.latency_ms = Some(ms);
                u.last_check = Some(now);
                transition(
                    true,
                    &mut u.ok_streak,
                    &mut u.fail_streak,
                    thr_ok,
                    thr_fail,
                    &mut u.status,
                )
            });
            if recovered == Some(Transition::ToUp) {
                ctx.log(
                    LogLevel::Ok,
                    format!("✅ 上游 [{name}] 恢复 UP（连续 {thr_ok} 次成功），延迟 {ms} ms"),
                );
            }
        }
        Err(reason) => {
            // prev 是"本次失败前"的状态，必须在 transition() 之前捕获，
            // 顺序不可颠倒，否则 UP 下首败判定失效。
            let state_change = ctx.state.update(idx, |u| {
                let prev = u.status;
                u.last_check = Some(now);
                let t = transition(
                    false,
                    &mut u.ok_streak,
                    &mut u.fail_streak,
                    thr_ok,
                    thr_fail,
                    &mut u.status,
                );
                (t, prev, u.fail_streak)
            });
            if let Some((t, prev, fail_streak)) = state_change {
                // DOWN 心跳节流：按检查间隔折算出「约每 60s 一条」的失败次数
                let warn_every = (60 / ctx.cfg.health.interval_secs.max(1)).max(1) as u32;
                match failure_log_level(t, prev, fail_streak, warn_every) {
                    Some(LogLevel::Error) => ctx.log(
                        LogLevel::Error,
                        format!(
                            "⛔ 上游 [{name}] 健康检查连续失败 {thr_fail} 次 → DOWN（{reason}）"
                        ),
                    ),
                    // 手动测试已有 🔍 结果日志，Warn 抑制（既有行为）
                    Some(LogLevel::Warn) if !manual => {
                        if prev == HealthStatus::Down {
                            ctx.log(
                                LogLevel::Warn,
                                format!(
                                    "上游 [{name}] 健康检查持续失败：已连续 {fail_streak} 次（{reason}）"
                                ),
                            );
                        } else {
                            ctx.log(
                                LogLevel::Warn,
                                format!("上游 [{name}] 健康检查失败（{reason}）"),
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const THR_OK: u32 = 2;
    const THR_FAIL: u32 = 2;

    fn step(
        ok: bool,
        ok_streak: &mut u32,
        fail_streak: &mut u32,
        status: &mut HealthStatus,
    ) -> Transition {
        transition(ok, ok_streak, fail_streak, THR_OK, THR_FAIL, status)
    }

    #[test]
    fn needs_two_consecutive_successes_to_recover() {
        let (mut ok_s, mut fail_s) = (0, 0);
        let mut st = HealthStatus::Down;
        assert_eq!(
            step(true, &mut ok_s, &mut fail_s, &mut st),
            Transition::None
        );
        assert_eq!(st, HealthStatus::Down);
        assert_eq!(
            step(true, &mut ok_s, &mut fail_s, &mut st),
            Transition::ToUp
        );
        assert_eq!(st, HealthStatus::Up);
    }

    #[test]
    fn needs_two_consecutive_failures_to_mark_down() {
        let (mut ok_s, mut fail_s) = (0, 0);
        let mut st = HealthStatus::Up;
        assert_eq!(
            step(false, &mut ok_s, &mut fail_s, &mut st),
            Transition::None
        );
        assert_eq!(st, HealthStatus::Up);
        assert_eq!(
            step(false, &mut ok_s, &mut fail_s, &mut st),
            Transition::ToDown
        );
        assert_eq!(st, HealthStatus::Down);
    }

    #[test]
    fn mixed_results_reset_the_other_streak() {
        let (mut ok_s, mut fail_s) = (0, 0);
        let mut st = HealthStatus::Up;
        step(false, &mut ok_s, &mut fail_s, &mut st);
        step(true, &mut ok_s, &mut fail_s, &mut st);
        assert_eq!(fail_s, 0);
        assert_eq!(ok_s, 1);
        // 一次成功只让失败计数清零，不会意外恢复为 DOWN 的上游
        assert_eq!(st, HealthStatus::Up);
    }

    #[test]
    fn success_keeps_latency_bookkeeping_without_transition() {
        let (mut ok_s, mut fail_s) = (5, 0);
        let mut st = HealthStatus::Up;
        assert_eq!(
            step(true, &mut ok_s, &mut fail_s, &mut st),
            Transition::None
        );
        assert_eq!(ok_s, 6);
    }

    #[test]
    fn failure_log_policy() {
        const EVERY: u32 = 7;
        // 转 DOWN → Error（手动/周期一致）
        assert_eq!(
            failure_log_level(Transition::ToDown, HealthStatus::Up, 2, EVERY),
            Some(LogLevel::Error)
        );
        // UP 下首败 → Warn
        assert_eq!(
            failure_log_level(Transition::None, HealthStatus::Up, 1, EVERY),
            Some(LogLevel::Warn)
        );
        // DOWN 后非心跳轮次的重复失败 → 静默
        assert_eq!(
            failure_log_level(Transition::None, HealthStatus::Down, 1, EVERY),
            None
        );
        assert_eq!(
            failure_log_level(Transition::None, HealthStatus::Down, 8, EVERY),
            None
        );
        // Unknown（启动未成功过）期间首败 → 静默
        assert_eq!(
            failure_log_level(Transition::None, HealthStatus::Unknown, 1, EVERY),
            None
        );
    }

    #[test]
    fn down_heartbeat_warns_on_multiples_of_warn_every() {
        const EVERY: u32 = 7;
        // DOWN 心跳：每逢 warn_every 整数倍打一条摘要 Warn
        assert_eq!(
            failure_log_level(Transition::None, HealthStatus::Down, 7, EVERY),
            Some(LogLevel::Warn)
        );
        assert_eq!(
            failure_log_level(Transition::None, HealthStatus::Down, 14, EVERY),
            Some(LogLevel::Warn)
        );
        assert_eq!(
            failure_log_level(Transition::None, HealthStatus::Down, 13, EVERY),
            None
        );
    }

    #[test]
    fn warn_every_zero_falls_back_to_one() {
        // warn_every=0（异常配置）按 1 处理：每次失败都提示，不 panic
        assert_eq!(
            failure_log_level(Transition::None, HealthStatus::Down, 1, 0),
            Some(LogLevel::Warn)
        );
        assert_eq!(
            failure_log_level(Transition::None, HealthStatus::Down, 2, 0),
            Some(LogLevel::Warn)
        );
    }
}
