//! 独立日志页：整页日志浏览，支持关键字与级别筛选。
//!
//! 自 cards.rs 的日志卡延伸——排查问题时需要更大的阅读面与过滤能力；
//! 由日志卡 ↗ 进入，「← 返回」或 Esc 回到主界面。数据仍是引擎的内存
//! 环形缓冲（完整历史在日志文件里），筛选只作用于显示与复制。

use eframe::egui;

use super::App;
use super::theme::{GRAY, GREEN, RED, SZ_SMALL, SZ_TINY, SZ_TITLE, YELLOW, palette};
use super::widgets::{ButtonVariant, card, icon_button, log_line, styled_button_ex};
use crate::engine::{LogLevel, Snapshot};

/// 级别筛选：四档独立开关，默认全部点亮。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct LevelFilter {
    pub(crate) info: bool,
    pub(crate) ok: bool,
    pub(crate) warn: bool,
    pub(crate) error: bool,
}

impl Default for LevelFilter {
    fn default() -> Self {
        Self {
            info: true,
            ok: true,
            warn: true,
            error: true,
        }
    }
}

impl LevelFilter {
    fn allows(self, level: LogLevel) -> bool {
        match level {
            LogLevel::Info => self.info,
            LogLevel::Ok => self.ok,
            LogLevel::Warn => self.warn,
            LogLevel::Error => self.error,
        }
    }

    fn all_on(self) -> bool {
        self.info && self.ok && self.warn && self.error
    }
}

impl App {
    pub(crate) fn logs_view(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let p = palette(self.dark);
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_logs = false;
        }

        // 先算筛选结果：标题行的「复制」与筛选行的计数、日志体共用同一份
        let query = self.log_search.trim().to_lowercase();
        let shown: Vec<_> = snap
            .logs
            .iter()
            .filter(|e| {
                self.log_levels.allows(e.level)
                    && (query.is_empty()
                        || e.msg.to_lowercase().contains(&query)
                        || e.time.contains(&query))
            })
            .collect();
        let filtered = !query.is_empty() || !self.log_levels.all_on();

        ui.horizontal(|ui| {
            if icon_button(ui, "←", "返回主界面 (Esc)", self.dark).clicked() {
                self.show_logs = false;
            }
            ui.label(egui::RichText::new("日志").strong().size(SZ_TITLE));
            ui.label(
                egui::RichText::new(format!(
                    "内存保留最近 {} 条 · 完整历史见日志文件",
                    snap.logs.len()
                ))
                .size(SZ_SMALL)
                .color(p.weak),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if styled_button_ex(ui, "清空", ButtonVariant::Ghost, self.dark, 24.0)
                    .on_hover_text("清空内存中的全部日志")
                    .clicked()
                {
                    self.engine.clear_logs();
                    self.notify(("日志已清空".into(), GREEN));
                }
                if styled_button_ex(ui, "复制", ButtonVariant::Ghost, self.dark, 24.0)
                    .on_hover_text("复制当前筛选结果")
                    .clicked()
                {
                    let text = shown
                        .iter()
                        .map(|e| e.line())
                        .collect::<Vec<_>>()
                        .join("\n");
                    ui.ctx().copy_text(text);
                    self.notify((format!("已复制 {} 条日志到剪贴板", shown.len()), GREEN));
                }
            });
        });
        ui.add_space(6.0);

        // 筛选卡：关键字 + 级别开关 + 命中计数
        card(ui, p.card, p.card_stroke, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.log_search)
                        .hint_text("按关键字过滤消息内容…")
                        .id_salt("log-search")
                        .desired_width(ui.available_width())
                        .font(egui::TextStyle::Monospace),
                );
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                level_toggle(ui, self.dark, "INFO", GRAY, &mut self.log_levels.info);
                level_toggle(ui, self.dark, "OK", GREEN, &mut self.log_levels.ok);
                level_toggle(ui, self.dark, "WARN", YELLOW, &mut self.log_levels.warn);
                level_toggle(ui, self.dark, "ERR", RED, &mut self.log_levels.error);
                if filtered
                    && styled_button_ex(ui, "重置", ButtonVariant::Ghost, self.dark, 22.0)
                        .on_hover_text("恢复显示全部级别并清除关键字")
                        .clicked()
                {
                    self.log_search.clear();
                    self.log_levels = LevelFilter::default();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "显示 {} / {} 条",
                            shown.len(),
                            snap.logs.len()
                        ))
                        .size(SZ_TINY)
                        .color(p.weak),
                    );
                });
            });
        });
        ui.add_space(6.0);

        // 日志体：填满页脚之上的全部余量（预算规则与设置页一致）
        let body_h = Self::page_body_height(ui, 60.0);
        egui::ScrollArea::vertical()
            .id_salt("logs-page")
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .max_height(body_h)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                if shown.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(28.0);
                        ui.label(
                            egui::RichText::new("没有匹配的日志")
                                .size(SZ_SMALL)
                                .color(p.weak),
                        );
                        ui.label(
                            egui::RichText::new("试试调整关键字或点亮更多级别")
                                .size(SZ_TINY)
                                .color(p.weak),
                        );
                    });
                }
                for e in &shown {
                    log_line(ui, e);
                }
            });
    }
}

/// 级别筛选开关：点亮 = 该级别参与显示，配色与日志行一致。
fn level_toggle(ui: &mut egui::Ui, dark: bool, label: &str, color: egui::Color32, on: &mut bool) {
    let p = palette(dark);
    let (fill, stroke, text) = if *on {
        (
            color.gamma_multiply(0.2),
            egui::Stroke::new(1.0, color.gamma_multiply(0.6)),
            p.text,
        )
    } else {
        (
            egui::Color32::TRANSPARENT,
            egui::Stroke::new(1.0, p.card_stroke),
            p.weak,
        )
    };
    let resp = ui.add(
        egui::Button::new(egui::RichText::new(label).size(10.5).strong().color(text))
            .fill(fill)
            .stroke(stroke)
            .corner_radius(egui::CornerRadius::same(6))
            .min_size(egui::vec2(44.0, 22.0)),
    );
    if resp.clicked() {
        *on = !*on;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_filter_default_allows_everything() {
        let f = LevelFilter::default();
        for lv in [
            LogLevel::Info,
            LogLevel::Ok,
            LogLevel::Warn,
            LogLevel::Error,
        ] {
            assert!(f.allows(lv));
        }
        assert!(f.all_on());
    }

    #[test]
    fn level_filter_toggles_levels_independently() {
        let mut f = LevelFilter {
            error: false,
            ..LevelFilter::default()
        };
        assert!(!f.allows(LogLevel::Error));
        assert!(f.allows(LogLevel::Warn));
        assert!(!f.all_on());
        f.warn = false;
        assert!(!f.allows(LogLevel::Warn));
        f.error = true;
        assert!(f.allows(LogLevel::Error));
    }
}
