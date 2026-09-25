//! 引擎与 GUI 之间共享的状态：快照、日志、上游运行信息与引擎上下文。
//! 日志落盘见 [`super::filelog`]。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::config::Config;

use super::filelog::FileLogger;
use super::upstream::ResolvedKind;

const LOG_CAP: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Ok,
    Warn,
    Error,
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            LogLevel::Info => "INFO",
            LogLevel::Ok => "OK",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERR ",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    Unknown,
    Up,
    Down,
}

impl HealthStatus {
    pub fn label(self) -> &'static str {
        match self {
            HealthStatus::Unknown => "未知",
            HealthStatus::Up => "健康",
            HealthStatus::Down => "故障",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Stopped,
    Starting,
    Running,
    BindFailed,
    Stopping,
}

#[derive(Clone)]
pub struct UpstreamState {
    pub name: String,
    pub status: HealthStatus,
    pub detected: Option<String>,
    pub latency_ms: Option<u64>,
    pub last_check: Option<String>,
    pub fail_streak: u32,
    pub ok_streak: u32,
    /// 累计转发的连接数与上下行字节数（自引擎本次启动起）
    pub conns: u64,
    pub bytes_up: u64,
    pub bytes_down: u64,
    /// 手动连通性测试进行中（GUI 据此显示等待动画）
    pub testing: bool,
}

/// 单个上游的累计流量计数器（无锁读写，按配置索引对齐）。
#[derive(Default)]
pub struct UpstreamStats {
    conns: AtomicU64,
    bytes_up: AtomicU64,
    bytes_down: AtomicU64,
}

#[derive(Clone)]
pub struct LogEntry {
    pub time: String,
    pub level: LogLevel,
    pub msg: String,
}

#[derive(Clone)]
pub struct Snapshot {
    pub phase: Phase,
    pub listen: String,
    pub error: Option<String>,
    pub upstreams: Vec<UpstreamState>,
    pub active: Option<String>,
    pub logs: Vec<LogEntry>,
}

/// 引擎状态的唯一持有者，GUI 与异步任务都通过它读写。
pub struct StateStore {
    snap: Mutex<Snapshot>,
    stats: Vec<UpstreamStats>,
    /// 每个上游「进行中的手动测试」计数（并发测试可重叠，归零才算结束）
    testing: Vec<AtomicUsize>,
    /// 日志文件写入器；独立锁，文件 IO 不与快照锁互相放大竞争
    file: Mutex<FileLogger>,
}

impl StateStore {
    pub fn new(cfg: &Config) -> Self {
        let upstreams = cfg
            .upstreams
            .iter()
            .map(|u| UpstreamState {
                name: u.name.clone(),
                status: HealthStatus::Unknown,
                detected: None,
                latency_ms: None,
                last_check: None,
                fail_streak: 0,
                ok_streak: 0,
                conns: 0,
                bytes_up: 0,
                bytes_down: 0,
                testing: false,
            })
            .collect();
        let stats = cfg
            .upstreams
            .iter()
            .map(|_| UpstreamStats::default())
            .collect();
        let testing = cfg.upstreams.iter().map(|_| AtomicUsize::new(0)).collect();
        StateStore {
            snap: Mutex::new(Snapshot {
                phase: Phase::Stopped,
                listen: cfg.general.listen.clone(),
                error: None,
                upstreams,
                active: None,
                logs: Vec::new(),
            }),
            stats,
            testing,
            file: Mutex::new(FileLogger::open(crate::config::logs_dir())),
        }
    }

    /// 隧道建立时 +1。
    pub fn record_conn(&self, idx: usize) {
        if let Some(s) = self.stats.get(idx) {
            s.conns.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 连接结束时累计上下行字节数。
    pub fn record_traffic(&self, idx: usize, up: u64, down: u64) {
        if let Some(s) = self.stats.get(idx) {
            s.bytes_up.fetch_add(up, Ordering::Relaxed);
            s.bytes_down.fetch_add(down, Ordering::Relaxed);
        }
    }

    /// 标记一次手动测试开始（可重叠）。
    pub fn begin_testing(&self, idx: usize) {
        if let Some(t) = self.testing.get(idx) {
            t.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 标记一次手动测试结束；计数归零后 GUI 的等待动画消失。
    pub fn end_testing(&self, idx: usize) {
        if let Some(t) = self.testing.get(idx) {
            let _ = t.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| {
                Some(c.saturating_sub(1))
            });
        }
    }

    pub fn log(&self, level: LogLevel, msg: impl Into<String>) {
        let msg = msg.into();
        let time = now_string();
        println!("[{time}] [{level}] {msg}");
        {
            let mut snap = self.snap.lock().unwrap();
            snap.logs.push(LogEntry {
                time: time.clone(),
                level,
                msg: msg.clone(),
            });
            let len = snap.logs.len();
            if len > LOG_CAP {
                snap.logs.drain(0..len - LOG_CAP);
            }
        }
        let date = chrono::Local::now().format("%F ");
        if let Ok(mut f) = self.file.lock() {
            f.write_line(&format!("{date}[{time}] [{level}] {msg}"));
        }
    }

    pub fn update<T>(&self, idx: usize, f: impl FnOnce(&mut UpstreamState) -> T) -> Option<T> {
        let mut snap = self.snap.lock().unwrap();
        let u = snap.upstreams.get_mut(idx)?;
        Some(f(u))
    }

    pub fn clear_logs(&self) {
        self.snap.lock().unwrap().logs.clear();
    }

    pub fn statuses(&self) -> Vec<HealthStatus> {
        self.snap
            .lock()
            .unwrap()
            .upstreams
            .iter()
            .map(|u| u.status)
            .collect()
    }

    /// 记录最近一次成功服务的上游；返回值表示「活跃上游是否发生变化」。
    pub fn set_active(&self, name: &str) -> bool {
        let mut snap = self.snap.lock().unwrap();
        let changed = snap.active.as_deref() != Some(name);
        if changed {
            snap.active = Some(name.to_string());
        }
        changed
    }

    pub fn set_phase(&self, phase: Phase, listen: &str, error: Option<String>) {
        let mut snap = self.snap.lock().unwrap();
        snap.phase = phase;
        snap.listen = listen.to_string();
        snap.error = error;
    }

    pub fn phase(&self) -> Phase {
        self.snap.lock().unwrap().phase
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut snap = self.snap.lock().unwrap().clone();
        for (i, u) in snap.upstreams.iter_mut().enumerate() {
            if let Some(s) = self.stats.get(i) {
                u.conns = s.conns.load(Ordering::Relaxed);
                u.bytes_up = s.bytes_up.load(Ordering::Relaxed);
                u.bytes_down = s.bytes_down.load(Ordering::Relaxed);
            }
            u.testing = self
                .testing
                .get(i)
                .map(|t| t.load(Ordering::Relaxed) > 0)
                .unwrap_or(false);
        }
        snap
    }
}

pub fn now_string() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

/// 引擎异步任务共享的上下文：配置、状态存储、健康检查唤醒通道与探测缓存。
pub struct EngineCtx {
    pub cfg: Arc<Config>,
    pub state: Arc<StateStore>,
    pub health_tx: mpsc::UnboundedSender<()>,
    pub kind_cache: Mutex<HashMap<String, ResolvedKind>>,
    pub url_warned: AtomicBool,
}

impl EngineCtx {
    pub fn log(&self, level: LogLevel, msg: impl Into<String>) {
        self.state.log(level, msg);
    }

    /// 被动感知到上游不可用时立即标记 DOWN（滞后阈值之外的快速路径）。
    /// 返回状态是否发生了变化，用于控制日志噪音。
    pub fn mark_down(&self, idx: usize) -> bool {
        let thr = self.cfg.health.fail_threshold.max(1);
        self.state
            .update(idx, |u| {
                let changed = u.status != HealthStatus::Down;
                u.status = HealthStatus::Down;
                u.ok_streak = 0;
                u.fail_streak = u.fail_streak.max(thr);
                changed
            })
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::StateStore;
    use crate::config::Config;

    #[test]
    fn manual_testing_flag_survives_overlapping_tests() {
        let store = StateStore::new(&Config::default());
        assert!(!store.snapshot().upstreams[0].testing);

        // 「测试全部」与单点测试重叠：计数归零才算结束
        store.begin_testing(0);
        store.begin_testing(0);
        store.end_testing(0);
        assert!(store.snapshot().upstreams[0].testing);

        store.end_testing(0);
        store.end_testing(0); // 多余的 end 不应下溢出负数
        assert!(!store.snapshot().upstreams[0].testing);
    }
}
