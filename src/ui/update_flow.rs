//! 自动更新的 UI 编排：检查/下载后台线程、事件回流与状态机。
//!
//! 自 ui.rs 收编为一个关注点：[`App`] 只保留本模块操作的字段
//! （`update_tx/rx/progress/pending/manual` 与 [`UpdateUi`] 状态机），
//! 下载/校验/替换的实际逻辑在 [`crate::update`]。

use eframe::egui;

use super::App;
use super::theme;

/// 更新流程的 UI 状态（与后台线程事件共同驱动）
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpdateUi {
    Idle,
    Checking,
    Downloading,
    Ready,
}

impl App {
    /// 下载通道：引擎运行时优先经本网关自身（享受上游故障切换），否则直连
    fn update_gateway(&self) -> Option<String> {
        if self.engine.is_running() {
            // listen 形如 "127.0.0.1:8888" / "0.0.0.0:8888"，回环访问只取端口
            self.cfg
                .general
                .listen
                .rsplit_once(':')
                .map(|(_, port)| format!("127.0.0.1:{port}"))
        } else {
            None
        }
    }

    /// 触发一次版本检查（manual=false 为启动静默检查，失败不打扰用户）
    pub(crate) fn check_updates(&mut self, manual: bool) {
        if self.update_ui == UpdateUi::Checking || self.update_ui == UpdateUi::Downloading {
            return;
        }
        self.update_ui = UpdateUi::Checking;
        self.update_manual = manual;
        let tx = self.update_tx.clone();
        let gateway = self.update_gateway();
        let spawned = std::thread::Builder::new()
            .name("proxyone-update".into())
            .spawn(move || {
                let res = crate::update::latest_release(gateway.as_deref())
                    .map(|opt| opt.filter(|r| crate::update::is_newer(&r.tag)));
                if res.is_ok() {
                    crate::update::mark_checked();
                }
                let _ = tx.send(crate::update::Msg::Checked(res));
            });
        if let Err(e) = spawned {
            self.update_ui = UpdateUi::Idle;
            self.notify((format!("无法启动检查线程: {e}"), theme::RED));
        }
    }

    /// 下载已发现的新版本：经网关优先，失败自动回退直连
    pub(crate) fn start_download(&mut self) {
        let Some(rel) = self.update_pending.clone() else {
            return;
        };
        self.update_ui = UpdateUi::Downloading;
        let tx = self.update_tx.clone();
        let progress = self.update_progress.clone();
        let gateway = self.update_gateway();
        let spawned = std::thread::Builder::new()
            .name("proxyone-update".into())
            .spawn(move || {
                let result = if let Some(g) = &gateway {
                    crate::update::download(Some(g), &rel, &progress).or_else(|e| {
                        eprintln!("经网关 {g} 下载失败，回退直连: {e:#}");
                        crate::update::download(None, &rel, &progress)
                    })
                } else {
                    crate::update::download(None, &rel, &progress)
                };
                let _ = tx.send(crate::update::Msg::Ready(result));
            });
        if let Err(e) = spawned {
            self.update_ui = UpdateUi::Idle;
            self.notify((format!("无法启动下载线程: {e}"), theme::RED));
        }
    }

    /// 应用更新：恢复系统代理 → 停引擎 → 原子替换并分离启动新版本 → 退出。
    /// 新实例带 --updated 延迟绑定端口；系统代理与引擎由新实例按持久化意图恢复。
    pub(crate) fn finish_update(&mut self, ctx: &egui::Context) {
        if let Err(e) = crate::platform::sysproxy::disable(&self.cfg.general.listen) {
            eprintln!("更新前恢复系统代理失败: {e:#}");
        }
        self.engine.stop();
        if let Err(e) = crate::update::install() {
            // 替换失败：把引擎拉起来保住网关
            let cfg = self.cfg.clone();
            self.engine.start(cfg);
            self.notify((format!("应用更新失败: {e:#}"), theme::RED));
            return;
        }
        self.exiting = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// 回收后台线程事件并推进状态机。
    /// Checked 只在手动检查时提示失败/无更新，自动检查保持静默。
    pub(crate) fn drain_update_events(&mut self) {
        while let Ok(m) = self.update_rx.try_recv() {
            let manual = self.update_manual;
            match m {
                crate::update::Msg::Checked(Ok(Some(rel))) => {
                    self.update_ui = UpdateUi::Idle;
                    let tag = rel.tag.clone();
                    self.update_pending = Some(rel);
                    self.notify((format!("发现新版本 {tag}，可在设置页更新"), theme::GREEN));
                }
                crate::update::Msg::Checked(Ok(None)) => {
                    self.update_ui = UpdateUi::Idle;
                    self.update_pending = None;
                    if manual {
                        self.notify((
                            format!("已是最新版本 v{}", crate::update::CURRENT_VERSION),
                            theme::GREEN,
                        ));
                    }
                }
                crate::update::Msg::Checked(Err(e)) => {
                    self.update_ui = UpdateUi::Idle;
                    if manual {
                        self.notify((format!("检查更新失败: {e:#}"), theme::ORANGE));
                    }
                }
                crate::update::Msg::Ready(Ok(tag)) => {
                    self.update_ui = UpdateUi::Ready;
                    self.notify((format!("v{tag} 已下载就绪，重启后生效"), theme::GREEN));
                }
                crate::update::Msg::Ready(Err(e)) => {
                    self.update_ui = UpdateUi::Idle;
                    self.notify((format!("下载更新失败: {e:#}"), theme::RED));
                }
            }
        }
    }
}
