//! 流量速率采样：每秒从累计字节计数取增量，维护各上游的当前速率与峰值。
//!
//! 数据源是 [`StateStore`] 的累计字节原子计数（由数据面的 [`super::stream::CountingStream`]
//! 实时累加）；采样任务只做增量差分，峰值与会话同生命周期（引擎重启即清零，
//! GUI 的「清零统计」也会同步重置基准）。

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use super::state::{EngineCtx, now_string};

pub async fn run(ctx: Arc<EngineCtx>, mut stop: watch::Receiver<bool>) {
    let mut last: Vec<(u64, u64)> = (0..ctx.cfg.upstreams.len())
        .map(|i| ctx.state.traffic_totals(i))
        .collect();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = tick.tick() => sample(&ctx, &mut last),
        }
    }
}

/// 一个采样窗口：累计计数差分 → 当前速率；创新高则记峰值与时刻。
///
/// 计数回落（「清零统计」）时以清零点为新基准，避免清零后的流量被
/// 旧基准吞掉。无流量的窗口把当前速率归零，峰值保持不动。
fn sample(ctx: &EngineCtx, last: &mut [(u64, u64)]) {
    for (i, slot) in last.iter_mut().enumerate() {
        let now = ctx.state.traffic_totals(i);
        let d_up = if now.0 >= slot.0 {
            now.0 - slot.0
        } else {
            now.0
        };
        let d_down = if now.1 >= slot.1 {
            now.1 - slot.1
        } else {
            now.1
        };
        *slot = now;
        if d_up == 0 && d_down == 0 {
            ctx.state.update(i, |u| {
                u.rate_up = 0;
                u.rate_down = 0;
            });
            continue;
        }
        ctx.state.update(i, |u| {
            u.rate_up = d_up;
            u.rate_down = d_down;
            let mut renewed = false;
            if d_up > u.peak_up {
                u.peak_up = d_up;
                renewed = true;
            }
            if d_down > u.peak_down {
                u.peak_down = d_down;
                renewed = true;
            }
            if renewed {
                u.peak_at = Some(now_string());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::engine::state::StateStore;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;

    fn ctx() -> Arc<EngineCtx> {
        let cfg = Config::default();
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
    fn window_delta_becomes_rate_and_peak() {
        let ctx = ctx();
        ctx.state.record_traffic(0, 500, 1500);
        let mut last = vec![(0u64, 0u64); ctx.cfg.upstreams.len()];
        sample(&ctx, &mut last);
        let u = &ctx.state.snapshot().upstreams[0];
        assert_eq!((u.rate_up, u.rate_down), (500, 1500));
        assert_eq!((u.peak_up, u.peak_down), (500, 1500));
        assert!(u.peak_at.is_some());

        // 无流量窗口：速率归零，峰值保留
        sample(&ctx, &mut last);
        let u = &ctx.state.snapshot().upstreams[0];
        assert_eq!((u.rate_up, u.rate_down), (0, 0));
        assert_eq!((u.peak_up, u.peak_down), (500, 1500));

        // 更高窗口刷新峰值
        ctx.state.record_traffic(0, 800, 100);
        sample(&ctx, &mut last);
        let u = &ctx.state.snapshot().upstreams[0];
        assert_eq!(u.peak_up, 800);
        assert_eq!(u.peak_down, 1500);
    }

    #[test]
    fn rebaselines_after_counter_reset() {
        let ctx = ctx();
        ctx.state.record_traffic(0, 1000, 1000);
        let mut last = vec![(0u64, 0u64); ctx.cfg.upstreams.len()];
        sample(&ctx, &mut last);

        ctx.state.reset_stats();
        ctx.state.record_traffic(0, 300, 200);
        sample(&ctx, &mut last);
        let u = &ctx.state.snapshot().upstreams[0];
        assert_eq!((u.rate_up, u.rate_down), (300, 200));
        assert_eq!((u.peak_up, u.peak_down), (300, 200));
    }
}
