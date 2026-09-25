//! 引擎生命周期：在后台线程中运行 tokio runtime，并提供启动/停止控制。

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};

use crate::config::Config;

use super::health;
use super::server;
use super::state::{EngineCtx, LogLevel, Phase, StateStore};

struct Worker {
    stop_tx: watch::Sender<bool>,
    done_rx: std::sync::mpsc::Receiver<()>,
}

pub struct EngineHandle {
    state: Arc<StateStore>,
    worker: Option<Worker>,
    test_tx: Option<mpsc::UnboundedSender<()>>,
}

impl EngineHandle {
    pub fn new(cfg: &Config) -> Self {
        EngineHandle {
            state: Arc::new(StateStore::new(cfg)),
            worker: None,
            test_tx: None,
        }
    }

    /// 触发一次全部上游的并发连通性测试（引擎运行中才有效）。
    pub fn test_all(&self) {
        if let Some(tx) = &self.test_tx {
            let _ = tx.send(());
        }
    }

    pub fn snapshot(&self) -> super::state::Snapshot {
        self.state.snapshot()
    }

    pub fn clear_logs(&self) {
        self.state.clear_logs();
    }

    pub fn is_running(&self) -> bool {
        matches!(self.state.phase(), Phase::Running | Phase::Starting)
    }

    pub fn start(&mut self, cfg: Config) {
        // 引擎线程可能已自行结束（例如端口绑定失败），此时允许重新启动。
        let finished = self
            .worker
            .as_ref()
            .map(|w| w.done_rx.try_recv().is_ok())
            .unwrap_or(false);
        if finished {
            self.worker = None;
        }
        if self.worker.is_some() {
            self.state.log(LogLevel::Warn, "引擎已在运行，忽略重复启动");
            return;
        }

        let cfg = Arc::new(cfg);
        self.state = Arc::new(StateStore::new(&cfg));
        let state = self.state.clone();
        state.log(
            LogLevel::Info,
            format!(
                "启动引擎：监听 {}，上游 {} 个",
                cfg.general.listen,
                cfg.upstreams.len()
            ),
        );

        let (stop_tx, stop_rx) = watch::channel(false);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let (test_tx, test_rx) = mpsc::unbounded_channel();
        let spawn_result = std::thread::Builder::new()
            .name("failgate-engine".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(r) => r,
                    Err(e) => {
                        state.log(LogLevel::Error, format!("tokio runtime 创建失败: {e}"));
                        let _ = done_tx.send(());
                        return;
                    }
                };
                rt.block_on(engine_main(cfg, state, test_rx, stop_rx));
                rt.shutdown_timeout(Duration::from_secs(1));
                let _ = done_tx.send(());
            });
        match spawn_result {
            Ok(_join) => {
                self.worker = Some(Worker { stop_tx, done_rx });
                self.test_tx = Some(test_tx);
            }
            Err(e) => {
                self.state
                    .log(LogLevel::Error, format!("引擎线程创建失败: {e}"));
            }
        }
    }

    /// 停止引擎并等待线程退出；线程卡死时最多阻塞 8 秒后放弃等待。
    pub fn stop(&mut self) {
        self.test_tx = None;
        if let Some(w) = self.worker.take() {
            let _ = w.stop_tx.send(true);
            let _ = w.done_rx.recv_timeout(Duration::from_secs(8));
        }
    }
}

async fn engine_main(
    cfg: Arc<Config>,
    state: Arc<StateStore>,
    test_rx: mpsc::UnboundedReceiver<()>,
    mut stop_rx: watch::Receiver<bool>,
) {
    state.set_phase(Phase::Starting, &cfg.general.listen, None);
    let listener = match TcpListener::bind(&cfg.general.listen).await {
        Ok(l) => l,
        Err(e) => {
            state.log(
                LogLevel::Error,
                format!("监听 {} 失败: {e}", cfg.general.listen),
            );
            state.set_phase(Phase::BindFailed, &cfg.general.listen, Some(format!("{e}")));
            return;
        }
    };
    let addr = listener
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| cfg.general.listen.clone());

    let (health_tx, health_rx) = mpsc::unbounded_channel();
    let ctx = Arc::new(EngineCtx {
        cfg: cfg.clone(),
        state: state.clone(),
        health_tx: health_tx.clone(),
        kind_cache: Mutex::new(HashMap::new()),
        url_warned: AtomicBool::new(false),
    });

    state.set_phase(Phase::Running, &addr, None);
    state.log(
        LogLevel::Ok,
        format!("✅ 开始监听 {addr}（HTTP + SOCKS5 混合入口）"),
    );
    if cfg.upstreams.is_empty() {
        state.log(LogLevel::Warn, "未配置任何上游，请求将全部失败");
    }

    let srv = tokio::spawn(server::run(listener, ctx.clone(), stop_rx.clone()));
    let hlth = tokio::spawn(health::run(
        ctx.clone(),
        health_rx,
        test_rx,
        stop_rx.clone(),
    ));
    let _ = health_tx.send(()); // 启动即做一轮健康检查

    let _ = stop_rx.changed().await;
    state.log(LogLevel::Info, "收到停止命令，正在退出…");
    state.set_phase(Phase::Stopping, &addr, None);
    srv.abort();
    hlth.abort();
    tokio::time::sleep(Duration::from_millis(150)).await;
    state.log(LogLevel::Info, "引擎已停止");
    state.set_phase(Phase::Stopped, &addr, None);
}
