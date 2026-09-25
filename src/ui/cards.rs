//! 主界面各视图与卡片的渲染：状态、上游列表、添加表单、日志与独立设置页。

use eframe::egui;
use std::sync::atomic::Ordering;

use super::theme::{
    GRAY, GREEN, ORANGE, RED, SZ_BODY, SZ_SECTION, SZ_SMALL, SZ_TINY, SZ_TITLE, YELLOW, palette,
};
use super::widgets::{
    ButtonVariant, card, chip, fmt_bytes, icon_button, level_color, styled_button,
    styled_button_ex, switch_row,
};
use super::{App, UpdateUi};
use crate::config::{self, UpstreamKind};
use crate::engine::{HealthStatus, Phase, Snapshot};

impl App {
    pub(crate) fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("proxyone").strong().size(SZ_TITLE));
            ui.label(
                egui::RichText::new("代理故障切换网关")
                    .weak()
                    .size(SZ_SMALL),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let theme_label = if self.dark { "☀" } else { "🌙" };
                let hover = if self.dark {
                    "切换到浅色主题"
                } else {
                    "切换到深色主题"
                };
                if icon_button(ui, theme_label, hover, self.dark).clicked() {
                    self.toggle_theme(ui.ctx());
                }
            });
        });
    }

    pub(crate) fn status_card(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let p = palette(self.dark);
        let (dot, label, running) = match snap.phase {
            Phase::Running => (GREEN, "运行中", true),
            Phase::Starting => (YELLOW, "启动中…", true),
            Phase::Stopping => (YELLOW, "停止中…", true),
            Phase::BindFailed => (RED, "端口绑定失败", false),
            Phase::Stopped => (GRAY, "已停止", false),
        };
        card(ui, p.card, p.card_stroke, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("●").color(dot).size(17.0));
                ui.label(egui::RichText::new(label).strong().size(SZ_TITLE));
                if let Some(e) = &snap.error {
                    ui.colored_label(RED, egui::RichText::new(e).size(SZ_SMALL));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if running {
                        if styled_button(ui, "■ 停止", ButtonVariant::Danger, self.dark).clicked()
                        {
                            self.engine.stop();
                        }
                    } else if styled_button(ui, "▶ 启动", ButtonVariant::Primary, self.dark)
                        .clicked()
                    {
                        let cfg = self.cfg.clone();
                        self.engine.start(cfg);
                    }
                });
            });
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("监听 {}", snap.listen))
                        .monospace()
                        .size(SZ_SMALL)
                        .color(p.weak),
                );
                ui.label(egui::RichText::new("·").color(p.weak));
                ui.label(egui::RichText::new("当前上游").size(SZ_SMALL).color(p.weak));
                chip(ui, snap.active.as_deref().unwrap_or("—"), GREEN, p.chip);
                if let Some((m, c)) = &self.msg {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(m).size(SZ_SMALL).color(*c));
                    });
                }
            });
            // 累计转发流量（所有上游合计）
            let (tot_up, tot_down, tot_conns) = snap
                .upstreams
                .iter()
                .fold((0u64, 0u64, 0u64), |(a, b, c), u| {
                    (a + u.bytes_up, b + u.bytes_down, c + u.conns)
                });
            if tot_conns > 0 {
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(format!(
                        "转发 ↑ {}  ↓ {}  ·  连接 {}",
                        fmt_bytes(tot_up),
                        fmt_bytes(tot_down),
                        tot_conns
                    ))
                    .size(SZ_TINY)
                    .color(p.weak),
                );
            }
        });
    }

    pub(crate) fn error_banner(&mut self, ui: &mut egui::Ui) {
        let Some(pe) = &self.parse_error else { return };
        let p = palette(self.dark);
        card(ui, p.card, RED, |ui| {
            ui.label(
                egui::RichText::new(format!("⚠ 配置解析失败，使用默认配置：{pe}"))
                    .size(SZ_SMALL)
                    .color(RED),
            );
        });
        ui.add_space(6.0);
    }

    pub(crate) fn section_row(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("上游 · {}", snap.upstreams.len()))
                    .strong()
                    .size(SZ_SECTION),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(ui, "⚙", "打开设置面板", self.dark).clicked() {
                    self.show_settings = true;
                }
                if styled_button_ex(ui, "＋ 添加", ButtonVariant::Ghost, self.dark, 24.0).clicked()
                {
                    self.show_add = !self.show_add;
                }
                if styled_button_ex(ui, "测试全部", ButtonVariant::Ghost, self.dark, 24.0)
                    .on_hover_text("并发测试所有上游连通性")
                    .clicked()
                {
                    if self.engine.is_running() {
                        self.engine.test_all();
                    } else {
                        self.notify(("引擎未运行，无法测试上游".into(), ORANGE));
                    }
                }
            });
        });
    }

    pub(crate) fn upstream_card(&mut self, ui: &mut egui::Ui, snap: &Snapshot, i: usize) {
        let p = palette(self.dark);
        let st = snap.upstreams.get(i);
        let is_active = st
            .map(|u| Some(u.name.as_str()) == snap.active.as_deref())
            .unwrap_or(false);
        let stroke = if is_active { GREEN } else { p.card_stroke };
        card(ui, p.card, stroke, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                let pri = self.cfg.upstreams[i].priority;
                chip(ui, &format!("P{pri}"), GREEN, p.chip);
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut self.cfg.upstreams[i].name)
                            .frame(egui::Frame::NONE)
                            .desired_width(78.0)
                            .font(egui::TextStyle::Body),
                    )
                    .changed()
                {
                    self.dirty = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if st.is_some_and(|u| u.testing) {
                        // 手动测试进行中：旋转等待动画替代延迟数字
                        ui.add(
                            egui::Spinner::new()
                                .size(13.0)
                                .color(palette(self.dark).text),
                        );
                        ui.label(
                            egui::RichText::new("测试中")
                                .size(SZ_SMALL)
                                .color(palette(self.dark).weak),
                        );
                    } else {
                        match st.and_then(|u| u.latency_ms) {
                            Some(ms) => {
                                let c = if ms < 800 {
                                    GREEN
                                } else if ms < 3000 {
                                    YELLOW
                                } else {
                                    RED
                                };
                                ui.label(
                                    egui::RichText::new(format!("{ms} ms"))
                                        .size(SZ_SMALL)
                                        .color(c),
                                );
                            }
                            None => {
                                ui.label(egui::RichText::new("— ms").size(SZ_SMALL).color(p.weak));
                            }
                        };
                    }
                    match st {
                        Some(u) => {
                            let (d, c) = match u.status {
                                HealthStatus::Up => ("●", GREEN),
                                HealthStatus::Down => ("●", RED),
                                HealthStatus::Unknown => ("○", GRAY),
                            };
                            ui.label(egui::RichText::new(d).color(c).size(SZ_SECTION));
                            ui.label(
                                egui::RichText::new(u.status.label())
                                    .size(SZ_SMALL)
                                    .color(p.weak),
                            );
                        }
                        None => {
                            ui.label(egui::RichText::new("未知").size(SZ_SMALL).color(p.weak));
                        }
                    }
                });
            });
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut self.cfg.upstreams[i].addr)
                            .frame(egui::Frame::NONE)
                            .desired_width(ui.available_width() - 132.0)
                            .font(egui::TextStyle::Monospace),
                    )
                    .changed()
                {
                    self.dirty = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, "×", "删除", self.dark).clicked() {
                        self.pending_action = Some(RowAction::Del(i));
                    }
                    if icon_button(ui, "↓", "下移", self.dark).clicked() {
                        self.pending_action = Some(RowAction::Down(i));
                    }
                    if icon_button(ui, "↑", "上移", self.dark).clicked() {
                        self.pending_action = Some(RowAction::Up(i));
                    }
                    let before = self.cfg.upstreams[i].kind;
                    egui::ComboBox::from_id_salt(format!("kind{i}"))
                        .selected_text(self.cfg.upstreams[i].kind.label())
                        .width(58.0)
                        .show_ui(ui, |ui| {
                            for k in [UpstreamKind::Auto, UpstreamKind::Http, UpstreamKind::Socks5]
                            {
                                ui.selectable_value(&mut self.cfg.upstreams[i].kind, k, k.label());
                            }
                        });
                    if self.cfg.upstreams[i].kind != before {
                        self.dirty = true;
                    }
                });
            });
            // 累计转发流量（连接数大于 0 时显示）
            if let Some(u) = st.filter(|u| u.conns > 0) {
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(format!(
                        "↑ {}  ↓ {}  ·  {} 次连接",
                        fmt_bytes(u.bytes_up),
                        fmt_bytes(u.bytes_down),
                        u.conns
                    ))
                    .size(SZ_TINY)
                    .color(p.weak),
                );
            }
        });
    }

    pub(crate) fn add_card(&mut self, ui: &mut egui::Ui) {
        let p = palette(self.dark);
        card(ui, p.card, p.card_stroke, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(egui::RichText::new("添加上游").strong().size(SZ_SECTION));
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("名称").size(SZ_SMALL).color(p.weak));
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_name)
                        .desired_width(90.0)
                        .font(egui::TextStyle::Body),
                );
                ui.label(egui::RichText::new("地址").size(SZ_SMALL).color(p.weak));
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_addr)
                        .desired_width(130.0)
                        .font(egui::TextStyle::Monospace),
                );
            });
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("类型").size(SZ_SMALL).color(p.weak));
                egui::ComboBox::from_id_salt("new-kind")
                    .selected_text(self.new_kind.label())
                    .width(58.0)
                    .show_ui(ui, |ui| {
                        for k in [UpstreamKind::Auto, UpstreamKind::Http, UpstreamKind::Socks5] {
                            ui.selectable_value(&mut self.new_kind, k, k.label());
                        }
                    });
                ui.label(egui::RichText::new("置顶").size(SZ_SMALL).color(p.weak));
                ui.checkbox(&mut self.new_top, "");
                if styled_button(ui, "添加", ButtonVariant::Primary, self.dark).clicked() {
                    self.submit_new_upstream();
                }
                if styled_button(ui, "取消", ButtonVariant::Ghost, self.dark).clicked() {
                    self.show_add = false;
                }
            });
        });
    }

    /// 独立设置页：由主界面 ⚙ 进入并覆盖整个内容区，「← 返回」或 Esc 回到主界面。
    /// 比旧的内联设置卡片空间宽裕，字段不再拥挤溢出。
    pub(crate) fn settings_view(&mut self, ui: &mut egui::Ui) {
        let p = palette(self.dark);
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_settings = false;
        }

        ui.horizontal(|ui| {
            if icon_button(ui, "←", "返回主界面 (Esc)", self.dark).clicked() {
                self.show_settings = false;
            }
            ui.label(egui::RichText::new("设置").strong().size(SZ_TITLE));
            if let Some((m, c)) = &self.msg {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(m).size(SZ_SMALL).color(*c));
                });
            }
        });
        ui.add_space(6.0);
        self.error_banner(ui);

        // ScrollArea 不吃 footer 的空间：预留下方 6px 间距 + 页脚预算高度（App::FOOTER_H）
        let scroll_h = (ui.available_height() - 6.0 - Self::FOOTER_H).max(120.0);
        egui::ScrollArea::vertical()
            .id_salt("settings")
            .auto_shrink(false)
            .max_height(scroll_h)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());

                card(ui, p.card, p.card_stroke, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.label(egui::RichText::new("通用").strong().size(SZ_SECTION));
                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("监听地址").size(SZ_SMALL).color(p.weak),
                        );
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut self.cfg.general.listen)
                                    .desired_width(ui.available_width())
                                    .font(egui::TextStyle::Monospace),
                            )
                            .changed()
                        {
                            self.dirty = true;
                        }
                    });
                    ui.add_space(6.0);
                    let mut on = self.dark;
                    if switch_row(ui, self.dark, "深色主题", "浅色 / 深色即时切换", &mut on) {
                        self.toggle_theme(ui.ctx());
                    }
                    ui.add_space(4.0);
                    let mut fwd = self.cfg.general.forward_log;
                    if switch_row(
                        ui,
                        self.dark,
                        "转发日志",
                        "记录每次连接的转发走向",
                        &mut fwd,
                    ) {
                        self.cfg.general.forward_log = fwd;
                        self.dirty = true;
                    }
                    ui.add_space(4.0);
                    let mut on = self.sysproxy_on;
                    if switch_row(
                        ui,
                        self.dark,
                        "系统代理",
                        "引擎运行时把 Windows 系统代理指向本网关",
                        &mut on,
                    ) {
                        self.set_sysproxy(on);
                    }
                    ui.add_space(4.0);
                    let mut on = self.autostart;
                    let autostart_toggled = switch_row(
                        ui,
                        self.dark,
                        "开机启动",
                        "登录时自动启动并隐藏到托盘",
                        &mut on,
                    );
                    if autostart_toggled {
                        self.set_autostart(!self.autostart);
                    }
                    ui.add_space(4.0);
                    let mut auto_upd = self.cfg.update.auto_check;
                    if switch_row(
                        ui,
                        self.dark,
                        "自动检查更新",
                        "启动时静默检查 GitHub 新版本（每 24h 至多一次）",
                        &mut auto_upd,
                    ) {
                        self.cfg.update.auto_check = auto_upd;
                        self.dirty = true;
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if styled_button_ex(
                            ui,
                            "打开日志目录",
                            ButtonVariant::Ghost,
                            self.dark,
                            24.0,
                        )
                        .clicked()
                        {
                            match crate::config::logs_dir() {
                                Some(d) => {
                                    let _ = std::fs::create_dir_all(&d);
                                    let _ = std::process::Command::new("explorer.exe")
                                        .arg(&d)
                                        .spawn();
                                }
                                None => {
                                    self.notify(("日志目录不可用".into(), ORANGE));
                                }
                            }
                        }
                        if styled_button_ex(
                            ui,
                            "打开配置所在目录",
                            ButtonVariant::Ghost,
                            self.dark,
                            24.0,
                        )
                        .clicked()
                        {
                            if self.cfg_path.parent().is_some() {
                                // explorer 恒返回非零码，只 spawn 不查结果
                                let _ = std::process::Command::new("explorer.exe")
                                    .arg(format!("/select,\"{}\"", self.cfg_path.display()))
                                    .spawn();
                            } else {
                                self.notify(("配置路径不可用".into(), ORANGE));
                            }
                        }
                    });
                });
                ui.add_space(8.0);

                card(ui, p.card, p.card_stroke, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.label(egui::RichText::new("健康检查").strong().size(SZ_SECTION));
                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("检查间隔").size(SZ_SMALL).color(p.weak),
                        );
                        if ui
                            .add_sized(
                                [64.0, 22.0],
                                egui::DragValue::new(&mut self.cfg.health.interval_secs)
                                    .range(1..=3600)
                                    .suffix("s"),
                            )
                            .changed()
                        {
                            self.dirty = true;
                        }
                        ui.label(egui::RichText::new("超时").size(SZ_SMALL).color(p.weak));
                        if ui
                            .add_sized(
                                [64.0, 22.0],
                                egui::DragValue::new(&mut self.cfg.health.timeout_secs)
                                    .range(1..=60)
                                    .suffix("s"),
                            )
                            .changed()
                        {
                            self.dirty = true;
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("连续失败→下线")
                                .size(SZ_SMALL)
                                .color(p.weak),
                        );
                        if ui
                            .add_sized(
                                [36.0, 22.0],
                                egui::DragValue::new(&mut self.cfg.health.fail_threshold)
                                    .range(1..=10),
                            )
                            .changed()
                        {
                            self.dirty = true;
                        }
                        ui.label(
                            egui::RichText::new("连续成功→恢复")
                                .size(SZ_SMALL)
                                .color(p.weak),
                        );
                        if ui
                            .add_sized(
                                [36.0, 22.0],
                                egui::DragValue::new(&mut self.cfg.health.success_threshold)
                                    .range(1..=10),
                            )
                            .changed()
                        {
                            self.dirty = true;
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("测试 URL").size(SZ_SMALL).color(p.weak),
                        );
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut self.cfg.health.test_url)
                                    .desired_width(ui.available_width())
                                    .font(egui::TextStyle::Monospace),
                            )
                            .changed()
                        {
                            self.dirty = true;
                        }
                    });
                });
                ui.add_space(8.0);

                card(ui, p.card, p.card_stroke, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.label(
                        egui::RichText::new("关于与更新").strong().size(SZ_SECTION),
                    );
                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "当前版本 v{}",
                                crate::update::CURRENT_VERSION
                            ))
                            .size(SZ_SMALL)
                            .color(p.weak),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            match self.update_ui {
                                UpdateUi::Idle => {
                                    if self.update_pending.is_some() {
                                        if styled_button(
                                            ui,
                                            "下载并更新",
                                            ButtonVariant::Primary,
                                            self.dark,
                                        )
                                        .clicked()
                                        {
                                            self.start_download();
                                        }
                                    } else if styled_button_ex(
                                        ui,
                                        "检查更新",
                                        ButtonVariant::Ghost,
                                        self.dark,
                                        24.0,
                                    )
                                    .clicked()
                                    {
                                        self.check_updates(true);
                                    }
                                }
                                UpdateUi::Checking => {
                                    ui.add(egui::Spinner::new().size(13.0).color(p.text));
                                    ui.label(
                                        egui::RichText::new("检查中").size(SZ_SMALL).color(p.weak),
                                    );
                                }
                                UpdateUi::Downloading => {
                                    ui.label(
                                        egui::RichText::new("下载中").size(SZ_SMALL).color(p.weak),
                                    );
                                }
                                UpdateUi::Ready => {
                                    if styled_button(
                                        ui,
                                        "重启并完成更新",
                                        ButtonVariant::Primary,
                                        self.dark,
                                    )
                                    .clicked()
                                    {
                                        let ctx = ui.ctx().clone();
                                        self.finish_update(&ctx);
                                    }
                                }
                            }
                        });
                    });
                    // 状态行：下载进度 / 就绪提示 / 新版本提示
                    match self.update_ui {
                        UpdateUi::Downloading => {
                            let dl = self.update_progress.downloaded.load(Ordering::Relaxed);
                            let total = self.update_progress.total.load(Ordering::Relaxed);
                            ui.add_space(4.0);
                            if total > 0 {
                                ui.add(
                                    egui::ProgressBar::new(
                                        (dl as f32 / total as f32).clamp(0.0, 1.0),
                                    )
                                    .show_percentage()
                                    .desired_width(ui.available_width()),
                                );
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} / {} · 优先经本网关下载（享受上游故障切换）",
                                        fmt_bytes(dl),
                                        fmt_bytes(total)
                                    ))
                                    .size(SZ_TINY)
                                    .color(p.weak),
                                );
                            } else {
                                ui.label(
                                    egui::RichText::new("准备下载…").size(SZ_SMALL).color(p.weak),
                                );
                            }
                        }
                        UpdateUi::Ready => {
                            let tag = self
                                .update_pending
                                .as_ref()
                                .map(|r| r.tag.clone())
                                .unwrap_or_default();
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(format!(
                                    "v{tag} 已就绪，点击「重启并完成更新」生效"
                                ))
                                .size(SZ_SMALL)
                                .color(GREEN),
                            );
                        }
                        UpdateUi::Idle if self.update_pending.is_some() => {
                            let tag = self
                                .update_pending
                                .as_ref()
                                .map(|r| r.tag.clone())
                                .unwrap_or_default();
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(format!("发现新版本 v{tag}"))
                                    .size(SZ_SMALL)
                                    .color(GREEN),
                            );
                        }
                        _ => {}
                    }
                });
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(
                        "上游按优先级取第一个存活者；监听/健康参数修改后需「保存并应用」重启引擎生效",
                    )
                    .size(SZ_TINY)
                    .color(p.weak),
                );
            });
    }

    pub(crate) fn log_card(&mut self, ui: &mut egui::Ui, snap: &Snapshot, body_max: f32) {
        let p = palette(self.dark);
        card(ui, p.card, p.card_stroke, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                let (arrow, hover) = if self.log_expanded {
                    ("▼", "收起日志")
                } else {
                    ("▲", "展开日志")
                };
                if icon_button(ui, arrow, hover, self.dark).clicked() {
                    self.log_expanded = !self.log_expanded;
                }
                ui.label(egui::RichText::new("日志").strong().size(SZ_BODY));
                ui.label(
                    egui::RichText::new(format!("最近 {} 条", snap.logs.len()))
                        .size(SZ_TINY)
                        .color(p.weak),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if styled_button(ui, "复制全部", ButtonVariant::Ghost, self.dark).clicked()
                    {
                        let text = snap
                            .logs
                            .iter()
                            .map(|e| format!("{} [{}] {}", e.time, e.level, e.msg))
                            .collect::<Vec<_>>()
                            .join("\n");
                        ui.ctx().copy_text(text);
                        self.notify(("日志已复制到剪贴板".into(), GREEN));
                    }
                    if styled_button(ui, "清空", ButtonVariant::Ghost, self.dark).clicked() {
                        self.engine.clear_logs();
                    }
                });
            });
            // 日志体取「卡片内实测剩余」与「外部预算上限」的较小者：
            // 折叠时上限 102，展开时填满预算空间，两个方向都不会把页脚挤出屏幕
            let avail_body = (ui.available_height() - 2.0).max(60.0);
            let body_h = avail_body.min(body_max);
            egui::ScrollArea::vertical()
                .id_salt("logs")
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .max_height(body_h)
                .show(ui, |ui| {
                    for e in &snap.logs {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("{} [{}] {}", e.time, e.level, e.msg))
                                    .monospace()
                                    .size(SZ_TINY)
                                    .color(level_color(e.level)),
                            )
                            .selectable(true),
                        );
                    }
                });
        });
    }

    /// 页脚：未保存标记 + 「保存并应用」。自带 8px 底部留白，
    /// 按钮永不贴住窗口下边缘；调用方的预算应预留 [`App::FOOTER_H`] 高度。
    pub(crate) fn footer(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if self.dirty {
                ui.label(
                    egui::RichText::new("● 有未保存修改")
                        .size(SZ_SMALL)
                        .color(ORANGE),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if styled_button(ui, "保存并应用", ButtonVariant::Primary, self.dark).clicked()
                {
                    self.save_and_apply();
                }
            });
        });
        ui.add_space(8.0);
    }

    fn submit_new_upstream(&mut self) {
        let name = self.new_name.trim().to_string();
        let addr = self.new_addr.trim().to_string();
        if name.is_empty() || addr.is_empty() || !addr.contains(':') {
            self.notify(("添加失败：名称与地址必填，地址需为 host:port".into(), RED));
            return;
        }
        let priority = if self.new_top {
            0
        } else {
            self.cfg.upstreams.len() as i32 + 1
        };
        self.cfg.upstreams.push(config::UpstreamConfig {
            name,
            addr,
            kind: self.new_kind,
            priority,
            username: None,
            password: None,
        });
        crate::ui::renumber_priorities(&mut self.cfg.upstreams);
        self.new_name.clear();
        self.new_addr.clear();
        self.show_add = false;
        self.dirty = true;
    }
}

#[derive(Clone, Copy)]
pub(crate) enum RowAction {
    Up(usize),
    Down(usize),
    Del(usize),
}
