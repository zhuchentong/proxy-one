//! 主界面各视图与卡片：状态、上游列表、添加表单、日志与页脚。
//! 独立设置页见 [`super::settings`]。

use eframe::egui;

use super::App;
use super::theme::{
    GRAY, GREEN, ORANGE, RED, SZ_BODY, SZ_SECTION, SZ_SMALL, SZ_TINY, SZ_TITLE, YELLOW, palette,
};
use super::widgets::{
    ButtonVariant, card, chip, icon_button, level_color, styled_button, styled_button_ex,
};
use crate::config::{self, UpstreamKind};
use crate::engine::{HealthStatus, Phase, Snapshot};
use crate::util::fmt_bytes;

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
