//! 独立统计页：按上游汇总健康状态、流量与速率峰值。
//!
//! 数据与主界面卡片同源（引擎快照），这里提供更完整的汇总视图：
//! 健康检查计数、累计流量、当前速率与峰值（1s 采样窗口）。
//! 由主界面「统计」进入，「← 返回」或 Esc 回到主界面。

use eframe::egui;

use super::App;
use super::theme::{GREEN, SZ_SECTION, SZ_SMALL, SZ_TITLE, palette};
use super::widgets::{
    ButtonVariant, card, chip, icon_button, latency_color, phase_indicator, status_glyph,
    styled_button_ex,
};
use crate::engine::{HealthStatus, Snapshot};
use crate::util::fmt_bytes;

impl App {
    pub(crate) fn stats_view(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let p = palette(self.dark);
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_stats = false;
        }

        ui.horizontal(|ui| {
            if icon_button(ui, "←", "返回主界面 (Esc)", self.dark).clicked() {
                self.show_stats = false;
            }
            ui.label(egui::RichText::new("统计").strong().size(SZ_TITLE));
            ui.label(
                egui::RichText::new("自引擎本次启动起 · 速率与峰值按每秒采样")
                    .size(SZ_SMALL)
                    .color(p.weak),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if styled_button_ex(ui, "清零统计", ButtonVariant::Ghost, self.dark, 24.0)
                    .on_hover_text("清空累计流量、速率、峰值与健康检查计数")
                    .clicked()
                {
                    self.engine.reset_stats();
                    self.notify(("统计已清零".into(), GREEN));
                }
            });
        });
        ui.add_space(6.0);
        self.error_banner(ui);

        // ScrollArea 不吃 footer 的空间：预留下方 6px 间距 + 页脚预算高度（App::FOOTER_H）
        let scroll_h = Self::page_body_height(ui, 120.0);
        egui::ScrollArea::vertical()
            .id_salt("stats")
            .auto_shrink(false)
            .max_height(scroll_h)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                self.summary_card(ui, snap, &p);
                ui.add_space(8.0);
                for i in 0..self.cfg.upstreams.len() {
                    self.upstream_stats_card(ui, snap, i, &p);
                    ui.add_space(8.0);
                }
                if self.cfg.upstreams.is_empty() {
                    ui.label(
                        egui::RichText::new("没有配置任何上游")
                            .size(SZ_SMALL)
                            .color(p.weak),
                    );
                }
            });
    }

    /// 汇总卡：运行状态、当前上游与全部上游合计的流量/速率。
    fn summary_card(&mut self, ui: &mut egui::Ui, snap: &Snapshot, p: &super::theme::Palette) {
        let (t_up, t_down, t_conns, r_up, r_down) = snap.traffic_totals();
        card(ui, p.card, p.card_stroke, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                let (dot, label) = phase_indicator(snap.phase);
                ui.label(egui::RichText::new("●").color(dot).size(15.0));
                ui.label(egui::RichText::new(label).strong().size(SZ_SECTION));
                ui.label(egui::RichText::new("当前上游").size(SZ_SMALL).color(p.weak));
                chip(ui, snap.active.as_deref().unwrap_or("—"), GREEN, p.chip);
            });
            ui.add_space(3.0);
            let mut line = format!(
                "累计 ↑ {}  ↓ {}  ·  连接 {}",
                fmt_bytes(t_up),
                fmt_bytes(t_down),
                t_conns
            );
            if r_up > 0 || r_down > 0 {
                line.push_str(&format!(
                    "  ·  当前 ↑ {}/s  ↓ {}/s",
                    fmt_bytes(r_up),
                    fmt_bytes(r_down)
                ));
            }
            ui.label(egui::RichText::new(line).size(SZ_SMALL).color(p.weak));
        });
    }

    /// 单上游统计卡：健康（状态/延迟/检查计数）、累计流量、速率与峰值。
    fn upstream_stats_card(
        &mut self,
        ui: &mut egui::Ui,
        snap: &Snapshot,
        i: usize,
        p: &super::theme::Palette,
    ) {
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
                ui.label(
                    egui::RichText::new(self.cfg.upstreams[i].name.clone())
                        .strong()
                        .size(SZ_SECTION),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    match st.and_then(|u| u.latency_ms) {
                        Some(ms) => {
                            let c = latency_color(ms);
                            ui.label(
                                egui::RichText::new(format!("{ms} ms"))
                                    .size(SZ_SMALL)
                                    .color(c),
                            );
                        }
                        None => {
                            ui.label(egui::RichText::new("— ms").size(SZ_SMALL).color(p.weak));
                        }
                    }
                    let (d, c) =
                        status_glyph(st.map(|u| u.status).unwrap_or(HealthStatus::Unknown));
                    ui.label(egui::RichText::new(d).color(c).size(SZ_SECTION));
                    ui.label(
                        egui::RichText::new(st.map(|u| u.status.label()).unwrap_or("未知"))
                            .size(SZ_SMALL)
                            .color(p.weak),
                    );
                });
            });
            ui.add_space(3.0);
            if let Some(u) = st {
                let mut health = match &u.last_check {
                    Some(t) => format!("上次检查 {t}"),
                    None => "尚未检查".into(),
                };
                if u.checks_total > 0 {
                    health.push_str(&format!(
                        "  ·  本轮检查 {} 次，失败 {} 次",
                        u.checks_total, u.checks_failed
                    ));
                }
                stat_line(ui, p, "健康", &health);
                stat_line(
                    ui,
                    p,
                    "累计",
                    &format!(
                        "↑ {}  ↓ {}  ·  {} 次连接",
                        fmt_bytes(u.bytes_up),
                        fmt_bytes(u.bytes_down),
                        u.conns
                    ),
                );
                let peak_at = u.peak_at.as_deref().unwrap_or("—");
                stat_line(
                    ui,
                    p,
                    "速率",
                    &format!(
                        "当前 ↑ {}/s  ↓ {}/s  ·  峰值 ↑ {}/s  ↓ {}/s（{peak_at}）",
                        fmt_bytes(u.rate_up),
                        fmt_bytes(u.rate_down),
                        fmt_bytes(u.peak_up),
                        fmt_bytes(u.peak_down),
                    ),
                );
            } else {
                ui.label(
                    egui::RichText::new("引擎尚未运行，暂无数据")
                        .size(SZ_SMALL)
                        .color(p.weak),
                );
            }
        });
    }
}

/// 统计行的统一排版：弱色字段名 + 数值。
fn stat_line(ui: &mut egui::Ui, p: &super::theme::Palette, name: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(name).size(SZ_SMALL).color(p.weak));
        ui.label(egui::RichText::new(value).size(SZ_SMALL));
    });
}
