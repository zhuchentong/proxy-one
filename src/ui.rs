//! GUI 层：eframe 应用、布局编排与用户交互。
//!
//! [`App`] 只负责状态与编排；具体卡片的绘制见 [`cards`]，
//! 主题配色见 [`theme`]，基础控件见 [`widgets`]。

mod cards;
mod settings;
mod theme;
mod update_flow;
pub(crate) mod widgets;

use eframe::egui;

use crate::config;
use crate::engine::EngineHandle;
use crate::platform::tray::{TrayHandle, TrayMsg};

pub use theme::{apply_theme, install_fonts};

use cards::RowAction;
use theme::palette;
use update_flow::UpdateUi;

pub struct App {
    pub(crate) engine: EngineHandle,
    pub(crate) cfg: config::Config,
    pub(crate) cfg_path: std::path::PathBuf,
    pub(crate) parse_error: Option<String>,
    pub(crate) dark: bool,
    pub(crate) show_add: bool,
    pub(crate) show_settings: bool,
    pub(crate) new_name: String,
    pub(crate) new_addr: String,
    pub(crate) new_kind: config::UpstreamKind,
    pub(crate) new_top: bool,
    pub(crate) pending_action: Option<RowAction>,
    pub(crate) autostart: bool,
    pub(crate) log_expanded: bool,
    pub(crate) dirty: bool,
    pub(crate) msg: Option<(String, egui::Color32)>,
    /// 消息过期时刻：超过后自动从状态卡消失
    pub(crate) msg_until: Option<std::time::Instant>,
    pub(crate) tray: Option<TrayHandle>,
    pub(crate) exiting: bool,
    /// 系统代理接管意图（持久化到 config，跨重启保持）
    pub(crate) sysproxy_on: bool,
    /// 系统代理同步节流：上次同步时间与强制同步标记
    pub(crate) sysproxy_last: Option<std::time::Instant>,
    pub(crate) sysproxy_resync: bool,
    /// 自动更新：后台事件通道、共享下载进度与 UI 状态机
    pub(crate) update_tx: std::sync::mpsc::Sender<crate::update::Msg>,
    pub(crate) update_rx: std::sync::mpsc::Receiver<crate::update::Msg>,
    pub(crate) update_progress: std::sync::Arc<crate::update::Progress>,
    pub(crate) update_ui: UpdateUi,
    pub(crate) update_pending: Option<crate::update::Release>,
    pub(crate) update_manual: bool,
}

impl App {
    pub fn new(loaded: config::LoadedConfig) -> Self {
        let dark = loaded.config.general.theme != "light";
        let sysproxy_on = loaded.config.general.sysproxy;
        let engine = EngineHandle::new(&loaded.config);
        let (update_tx, update_rx) = std::sync::mpsc::channel();
        let mut app = App {
            engine,
            cfg: loaded.config,
            cfg_path: loaded.path,
            parse_error: loaded.parse_error,
            dark,
            show_add: false,
            show_settings: false,
            new_name: String::new(),
            new_addr: String::new(),
            new_kind: config::UpstreamKind::Auto,
            new_top: false,
            pending_action: None,
            autostart: crate::platform::autostart::is_enabled(),
            log_expanded: false,
            dirty: false,
            msg: None,
            msg_until: None,
            tray: None,
            exiting: false,
            sysproxy_on,
            sysproxy_last: None,
            sysproxy_resync: false,
            update_tx,
            update_rx,
            update_progress: std::sync::Arc::new(crate::update::Progress::default()),
            update_ui: UpdateUi::Idle,
            update_pending: None,
            update_manual: false,
        };
        // 上次异常退出可能留下指向自己的系统代理：先恢复，再由同步逻辑按意图重开
        crate::platform::sysproxy::restore_pending(&app.cfg.general.listen);
        app.engine.start(app.cfg.clone());
        // 改名前残留的注册表旧值清理；exe 被移动/重命名后修正残留的旧路径
        crate::platform::autostart::remove_legacy();
        if crate::platform::autostart::is_enabled() && crate::platform::autostart::stale() {
            let _ = crate::platform::autostart::set_enabled(true);
        }
        // 上一代替换残留（.old/.new）清理；版本变化则提示「已更新」
        crate::update::cleanup_stale();
        if let Some(prev) = crate::update::take_version_change() {
            app.notify((
                format!("已更新到 v{}（原 v{prev}）", crate::update::CURRENT_VERSION),
                theme::GREEN,
            ));
        }
        // 每 24h 至多一次的启动静默检查
        if app.cfg.update.auto_check && crate::update::should_auto_check() {
            app.check_updates(false);
        }
        app
    }

    pub fn set_tray(&mut self, tray: TrayHandle) {
        self.tray = Some(tray);
    }

    /// 状态卡提示：6 秒后自动消失，不会长期驻留误导。
    pub(crate) fn notify(&mut self, msg: (String, egui::Color32)) {
        self.msg = Some(msg);
        self.msg_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(6));
    }

    pub(crate) fn save_and_apply(&mut self) {
        match config::save(&self.cfg_path, &self.cfg) {
            Ok(()) => self.notify(("已保存 config.toml".into(), theme::GREEN)),
            Err(e) => {
                self.notify((format!("保存失败: {e:#}"), theme::RED));
                return;
            }
        }
        self.dirty = false;
        if self.engine.is_running() {
            self.engine.stop();
            let cfg = self.cfg.clone();
            self.engine.start(cfg);
            self.notify(("已保存并应用（引擎已重启）".into(), theme::GREEN));
        }
    }

    pub(crate) fn toggle_theme(&mut self, ctx: &egui::Context) {
        self.dark = !self.dark;
        self.cfg.general.theme = if self.dark { "dark" } else { "light" }.into();
        apply_theme(ctx, self.dark);
        if let Err(e) = config::save(&self.cfg_path, &self.cfg) {
            self.notify((format!("主题保存失败: {e:#}"), theme::RED));
        }
    }

    pub(crate) fn set_autostart(&mut self, enable: bool) {
        match crate::platform::autostart::set_enabled(enable) {
            Ok(()) => {
                self.autostart = enable;
                self.notify((
                    if enable {
                        "已开启开机启动".into()
                    } else {
                        "已关闭开机启动".into()
                    },
                    theme::GREEN,
                ));
            }
            Err(e) => {
                self.autostart = crate::platform::autostart::is_enabled();
                self.notify((format!("开机启动设置失败: {e:#}"), theme::RED));
            }
        }
    }

    fn apply_pending_row_action(&mut self) {
        match self.pending_action.take() {
            Some(RowAction::Del(i)) => {
                self.cfg.upstreams.remove(i);
                renumber_priorities(&mut self.cfg.upstreams);
                self.dirty = true;
            }
            Some(RowAction::Up(i)) if i > 0 => {
                self.cfg.upstreams.swap(i, i - 1);
                renumber_priorities(&mut self.cfg.upstreams);
                self.dirty = true;
            }
            Some(RowAction::Down(i)) if i + 1 < self.cfg.upstreams.len() => {
                self.cfg.upstreams.swap(i, i + 1);
                renumber_priorities(&mut self.cfg.upstreams);
                self.dirty = true;
            }
            _ => {}
        }
    }
}

pub(crate) fn renumber_priorities(rows: &mut [config::UpstreamConfig]) {
    for (idx, r) in rows.iter_mut().enumerate() {
        r.priority = (idx + 1) as i32;
    }
}

impl eframe::App for App {
    /// 窗口隐藏时依然被调用的回调：处理托盘消息与关闭拦截。
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut msgs = Vec::new();
        if let Some(tray) = &self.tray {
            while let Some(m) = tray.try_recv() {
                msgs.push(m);
            }
        }
        for m in msgs {
            match m {
                TrayMsg::ShowWindow => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayMsg::ToggleEngine => {
                    if self.engine.is_running() {
                        self.engine.stop();
                    } else {
                        let cfg = self.cfg.clone();
                        self.engine.start(cfg);
                    }
                }
                TrayMsg::ToggleSysProxy => {
                    let on = !self.sysproxy_on;
                    self.set_sysproxy(on);
                }
                TrayMsg::TestAll => {
                    if self.engine.is_running() {
                        self.engine.test_all();
                        self.notify(("已从托盘触发全部上游测试".into(), theme::GREEN));
                    } else {
                        self.notify(("引擎未运行，无法测试上游".into(), theme::ORANGE));
                    }
                }
                TrayMsg::OpenConfig => {
                    if let Err(e) = crate::platform::desktop::edit_text_file(&self.cfg_path) {
                        self.notify((format!("打开配置文件失败: {e}"), theme::RED));
                    }
                }
                TrayMsg::Quit => {
                    self.exiting = true;
                    // 退出前恢复系统代理，避免留下指向死端口的代理
                    if let Err(e) = crate::platform::sysproxy::disable(&self.cfg.general.listen) {
                        eprintln!("退出恢复系统代理失败: {e:#}");
                    }
                    self.engine.stop();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if close_requested && !self.exiting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        // 过期提示自动清除，避免一条消息永远挂在状态卡上
        if let Some(until) = self.msg_until
            && std::time::Instant::now() > until
        {
            self.msg = None;
            self.msg_until = None;
        }
        // 托盘信息放在 logic() 刷新：窗口隐藏到托盘时依然每秒同步
        if let Some(tray) = &mut self.tray {
            let snap = self.engine.snapshot();
            tray.sync_snapshot(&snap);
            tray.set_sysproxy_checked(self.sysproxy_on);
        }
        // 系统代理声明式同步（1s 节流）：意图与实际注册表状态对齐
        let now = std::time::Instant::now();
        let due = self
            .sysproxy_last
            .is_none_or(|t| now.duration_since(t).as_millis() >= 1000);
        if self.sysproxy_resync || due {
            self.sysproxy_resync = false;
            self.sysproxy_last = Some(now);
            self.sync_sysproxy();
        }
        // 更新事件：Checked 只在手动检查时提示失败/无更新，自动检查保持静默
        self.drain_update_events();
        // 检查/下载进行中提高重绘频率，让进度条与转圈动画流动
        if self.update_ui == UpdateUi::Checking || self.update_ui == UpdateUi::Downloading {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let snap = self.engine.snapshot();

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(palette(self.dark).bg)
                    .inner_margin(egui::Margin::same(10)),
            )
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                if self.show_settings {
                    self.settings_view(ui);
                } else {
                    self.main_view(ui, &snap);
                }
                ui.add_space(6.0);
                self.footer(ui);
            });

        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }
}

impl App {
    /// 页脚预算高度：按钮 26 + footer 自带底部留白 8 + 富余 2。
    /// main_view 与 settings_view 共用，保证按钮与窗口下边缘保持呼吸空间。
    pub(crate) const FOOTER_H: f32 = 36.0;

    /// 系统代理开关：仅引擎运行时可开启；意图持久化到 config。
    fn set_sysproxy(&mut self, on: bool) {
        if on && !self.engine.is_running() {
            self.notify(("引擎未运行，无法接管系统代理".into(), theme::ORANGE));
            return;
        }
        let listen = self.cfg.general.listen.clone();
        let result = if on {
            crate::platform::sysproxy::enable(&listen)
        } else {
            crate::platform::sysproxy::disable(&listen)
        };
        match result {
            Ok(()) => {
                self.sysproxy_on = on;
                self.cfg.general.sysproxy = on;
                if let Err(e) = config::save(&self.cfg_path, &self.cfg) {
                    self.notify((format!("保存设置失败: {e:#}"), theme::RED));
                }
                self.sysproxy_resync = true;
                self.notify((
                    if on {
                        "系统代理已指向本网关".into()
                    } else {
                        "系统代理已恢复原状".into()
                    },
                    theme::GREEN,
                ));
            }
            Err(e) => {
                self.notify((format!("系统代理设置失败: {e:#}"), theme::RED));
            }
        }
    }

    /// 声明式同步：意图（sysproxy_on + 引擎运行）与注册表实际状态对齐。
    ///
    /// - 尚未接管（启动后首次 / 退出时自己还原过的原值环境）：按持久化意图
    ///   直接开启——注册表里的 8890 之类"别的代理"是我们自己恢复的，不是被抢；
    /// - 曾接管（快照存在）却被会话中的其他程序改走：才是真正的"被接管"，
    ///   此时放弃并关闭开关，避免与对方互相抢写。
    fn sync_sysproxy(&mut self) {
        let listen = self.cfg.general.listen.clone();
        let desired = self.sysproxy_on && self.engine.is_running();
        match (desired, crate::platform::sysproxy::is_active(&listen)) {
            (true, false) => {
                if crate::platform::sysproxy::has_snapshot() {
                    self.sysproxy_on = false;
                    self.cfg.general.sysproxy = false;
                    let _ = config::save(&self.cfg_path, &self.cfg);
                    self.notify((
                        "系统代理已被其他程序接管，已自动关闭系统代理开关".into(),
                        theme::ORANGE,
                    ));
                } else if let Err(e) = crate::platform::sysproxy::enable(&listen) {
                    self.notify((format!("开启系统代理失败: {e:#}"), theme::RED));
                }
            }
            (false, true) => {
                if let Err(e) = crate::platform::sysproxy::disable(&listen) {
                    self.notify((format!("恢复系统代理失败: {e:#}"), theme::RED));
                }
            }
            _ => {}
        }
    }

    /// 主界面：状态卡 + 上游列表 + 日志。
    fn main_view(&mut self, ui: &mut egui::Ui, snap: &crate::engine::Snapshot) {
        self.header(ui);
        ui.add_space(6.0);
        self.status_card(ui, snap);
        self.error_banner(ui);
        ui.add_space(8.0);
        self.section_row(ui, snap);
        ui.add_space(4.0);

        // 高度预算全部实测：remaining 是在状态卡/错误横幅之后量出的真实余量，
        // 下方依次是 卡片区 + 6 + 日志卡 + 6 + footer(App::FOOTER_H，含底部留白)，
        // 逐项扣减并留有富余，保证「保存并应用」任何窗口尺寸下都不贴窗口边缘。
        let remaining = ui.available_height();
        const SPACING: f32 = 12.0; // 卡片区/日志卡/页脚之间的两处 6px 间距
        const LOG_CHROME: f32 = 44.0; // 日志卡边距 + 标题行 + 日志体之外的部分
        let budget = remaining - Self::FOOTER_H - SPACING;
        let (cards_h, log_body_max) = if self.log_expanded {
            let cards = ((budget - LOG_CHROME) * 0.35).clamp(64.0, 150.0);
            (cards, (budget - cards - LOG_CHROME).max(60.0))
        } else {
            (
                (budget - LOG_CHROME - 102.0).max(80.0),
                102.0f32.min((budget - LOG_CHROME).max(60.0)),
            )
        };
        egui::ScrollArea::vertical()
            .id_salt("cards")
            .auto_shrink(false)
            .max_height(cards_h)
            .show(ui, |ui| {
                for i in 0..self.cfg.upstreams.len() {
                    self.upstream_card(ui, snap, i);
                    ui.add_space(6.0);
                }
                if self.show_add {
                    self.add_card(ui);
                    ui.add_space(6.0);
                }
            });
        self.apply_pending_row_action();

        ui.add_space(6.0);
        self.log_card(ui, snap, log_body_max);
    }
}
