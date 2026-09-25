//! 独立设置页：通用、健康检查与「关于与更新」三张卡片。
//!
//! 自 cards.rs 拆出——设置页是独立的整屏视图，与主界面卡片互不纠缠；
//! 由主界面 ⚙ 进入并覆盖整个内容区，「← 返回」或 Esc 回到主界面。

use std::sync::atomic::Ordering;

use eframe::egui;

use super::App;
use super::theme::{GREEN, ORANGE, SZ_SECTION, SZ_SMALL, SZ_TINY, SZ_TITLE, palette};
use super::update_flow::UpdateUi;
use super::widgets::{
    ButtonVariant, card, icon_button, styled_button, styled_button_ex, switch_row,
};
use crate::util::fmt_bytes;

impl App {
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
                self.general_card(ui, &p);
                ui.add_space(8.0);
                self.health_card(ui, &p);
                ui.add_space(8.0);
                self.about_card(ui, &p);
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

    /// 通用卡：监听、主题、转发日志、系统代理、开机启动、自动更新与快捷入口。
    fn general_card(&mut self, ui: &mut egui::Ui, p: &super::theme::Palette) {
        card(ui, p.card, p.card_stroke, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(egui::RichText::new("通用").strong().size(SZ_SECTION));
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("监听地址").size(SZ_SMALL).color(p.weak));
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
                "引擎运行时把系统代理指向本网关",
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
                if styled_button_ex(ui, "打开日志目录", ButtonVariant::Ghost, self.dark, 24.0)
                    .clicked()
                {
                    match crate::config::logs_dir() {
                        Some(d) => {
                            let _ = std::fs::create_dir_all(&d);
                            let _ = crate::platform::desktop::open_path(&d);
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
                        let _ = crate::platform::desktop::reveal_path(&self.cfg_path);
                    } else {
                        self.notify(("配置路径不可用".into(), ORANGE));
                    }
                }
            });
        });
    }

    /// 健康检查卡：间隔、超时、滞后阈值与测试 URL。
    fn health_card(&mut self, ui: &mut egui::Ui, p: &super::theme::Palette) {
        card(ui, p.card, p.card_stroke, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(egui::RichText::new("健康检查").strong().size(SZ_SECTION));
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("检查间隔").size(SZ_SMALL).color(p.weak));
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
                        egui::DragValue::new(&mut self.cfg.health.fail_threshold).range(1..=10),
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
                        egui::DragValue::new(&mut self.cfg.health.success_threshold).range(1..=10),
                    )
                    .changed()
                {
                    self.dirty = true;
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("测试 URL").size(SZ_SMALL).color(p.weak));
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
    }

    /// 关于与更新卡：版本号、检查/下载/应用更新的状态机呈现。
    fn about_card(&mut self, ui: &mut egui::Ui, p: &super::theme::Palette) {
        card(ui, p.card, p.card_stroke, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(egui::RichText::new("关于与更新").strong().size(SZ_SECTION));
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("当前版本 v{}", crate::update::CURRENT_VERSION))
                        .size(SZ_SMALL)
                        .color(p.weak),
                );
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| match self.update_ui {
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
                            ui.label(egui::RichText::new("检查中").size(SZ_SMALL).color(p.weak));
                        }
                        UpdateUi::Downloading => {
                            ui.label(egui::RichText::new("下载中").size(SZ_SMALL).color(p.weak));
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
                    },
                );
            });
            // 状态行：下载进度 / 就绪提示 / 新版本提示
            match self.update_ui {
                UpdateUi::Downloading => {
                    let dl = self.update_progress.downloaded.load(Ordering::Relaxed);
                    let total = self.update_progress.total.load(Ordering::Relaxed);
                    ui.add_space(4.0);
                    if total > 0 {
                        ui.add(
                            egui::ProgressBar::new((dl as f32 / total as f32).clamp(0.0, 1.0))
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
                            egui::RichText::new("准备下载…")
                                .size(SZ_SMALL)
                                .color(p.weak),
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
                        egui::RichText::new(format!("v{tag} 已就绪，点击「重启并完成更新」生效"))
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
    }
}
