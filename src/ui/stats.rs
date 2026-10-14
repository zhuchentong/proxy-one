//! 独立统计页：按上游汇总健康状态、流量与速率峰值。
//!
//! 数据与主界面卡片同源（引擎快照），这里提供更完整的汇总视图：
//! 健康检查计数、累计流量、当前速率与峰值（1s 采样窗口）。
//! 由主界面「统计」进入，「← 返回」或 Esc 回到主界面。

use eframe::egui;

use super::App;
use super::theme::{GREEN, SZ_NUM, SZ_SECTION, SZ_SMALL, SZ_TINY, SZ_TITLE, palette};
use super::widgets::{
    ButtonVariant, card, chip, latency_color, phase_indicator, rate_bar, status_glyph,
    styled_button_ex,
};
use crate::engine::{HealthStatus, Snapshot};
use crate::util::fmt_bytes;

/// 速率历史采样与热度分档：纯逻辑，与渲染解耦，可独立单测。
pub(crate) mod samples {
    use std::collections::VecDeque;
    use std::time::{Duration, Instant};

    /// 单个速率样本：记录时刻（用于 tooltip）与 ↑+↓ 合计速率（字节/秒）。
    #[derive(Debug, Clone)]
    pub(crate) struct Sample {
        pub(crate) at: chrono::DateTime<chrono::Local>,
        pub(crate) rate: u64,
    }

    /// 保留容量：900 样本 ≈ 最近 15 分钟（每秒 1 样本）。
    pub(crate) const SAMPLE_CAP: usize = 900;

    /// 热力图每行格数：60 格 × 15 行 = 完整窗口。
    pub(crate) const HEAT_COLS: usize = 60;

    /// 每帧采样（UI 每秒至少重绘一次，见 `ui::ui` 的 `request_repaint_after`）：
    ///
    /// 1. 回落检测：累计计数任一分量小于上次入样基准 → 视为引擎重启/清零，
    ///    清空历史、重置基准与节流，当帧不推样（累计计数正常只增不减）；
    /// 2. 1s 节流：距上次入样满 1s 才推入；节流失败帧基准不前进。
    ///    UI 与引擎的 1s 时钟相位不同步，属近似采样，偶发一格重复/空缺无碍；
    /// 3. 容量截断：超 [`SAMPLE_CAP`] 丢弃最旧。
    ///
    /// `totals` 是累计计数（回落检测基准），`rate` 是引擎侧 1s 差分速率
    /// （↑+↓ 字节/秒）。返回是否推入了新样本。
    pub(crate) fn push_sample(
        history: &mut VecDeque<Sample>,
        last: &mut Option<Instant>,
        last_totals: &mut (u64, u64),
        totals: (u64, u64),
        rate: u64,
        now: Instant,
    ) -> bool {
        if totals.0 < last_totals.0 || totals.1 < last_totals.1 {
            *last_totals = totals;
            *last = None;
            history.clear();
            return false;
        }
        if last.is_some_and(|t| now.duration_since(t) < Duration::from_secs(1)) {
            return false;
        }
        *last = Some(now);
        *last_totals = totals;
        if history.len() >= SAMPLE_CAP {
            history.pop_front();
        }
        history.push_back(Sample {
            at: chrono::Local::now(),
            rate,
        });
        true
    }

    /// 热度分档 0..=4：0=无流量；否则按 `rate / reference` 分 4 档
    /// （≤25% / ≤50% / ≤75% / >75%）。`reference` 为 0 → 0 档（防除零）。
    pub(crate) fn heat_level(rate: u64, reference: u64) -> u8 {
        if rate == 0 || reference == 0 {
            return 0;
        }
        let q = rate as f64 / reference as f64;
        if q <= 0.25 {
            1
        } else if q <= 0.5 {
            2
        } else if q <= 0.75 {
            3
        } else {
            4
        }
    }

    /// 迷你速率图的归一化坐标：取尾部 `n` 个样本，映射到 `x∈[0,1]`（左→右）、
    /// `y∈[0,1]`（0=顶部峰值，1=底部零速）。峰值 0（全零/空）时平线贴底防除零。
    pub(crate) fn sparkline_points(history: &VecDeque<Sample>, n: usize) -> Vec<(f32, f32)> {
        let start = history.len().saturating_sub(n);
        let slice: Vec<_> = history.iter().skip(start).collect();
        if slice.is_empty() {
            return Vec::new();
        }
        let peak = slice.iter().map(|s| s.rate).max().unwrap_or(0);
        let k = slice.len();
        slice
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let x = if k == 1 {
                    0.0
                } else {
                    i as f32 / (k - 1) as f32
                };
                let y = if peak == 0 {
                    1.0
                } else {
                    1.0 - s.rate as f32 / peak as f32
                };
                (x, y)
            })
            .collect()
    }

    /// 时间线格子悬停文本：每行一个样本，`时刻 ✓ 延迟` 或 `时刻 ✗`；空格返回 None。
    pub(crate) fn hover_lines(samples: &[crate::engine::CheckSample]) -> Option<String> {
        if samples.is_empty() {
            return None;
        }
        Some(
            samples
                .iter()
                .map(|s| match s.latency_ms {
                    Some(ms) => format!("{} ✓ {ms} ms", s.stamp),
                    None => format!("{} ✗", s.stamp),
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
}

/// 时间线格子配色：绿=全成功、红=全失败、黄=成败混合、弱底色=无数据。
fn cell_color(kind: &crate::engine::CellKind, p: &super::theme::Palette) -> egui::Color32 {
    match kind {
        crate::engine::CellKind::AllOk => super::theme::GREEN,
        crate::engine::CellKind::AllFail => super::theme::RED,
        crate::engine::CellKind::Mixed => super::theme::YELLOW,
        crate::engine::CellKind::Empty => p.field,
    }
}

impl App {
    /// 每帧采样入口：与页面开关无关，保证热力图始终是真正的「最近 15 分钟」。
    /// 回落检测基准用累计计数，样本 rate 用引擎侧 1s 差分速率（↑+↓）。
    pub(crate) fn sample_rates(&mut self, snap: &Snapshot) {
        let (acc_up, acc_down, _, r_up, r_down) = snap.traffic_totals();
        samples::push_sample(
            &mut self.rate_history,
            &mut self.last_sample,
            &mut self.last_totals,
            (acc_up, acc_down),
            r_up + r_down,
            std::time::Instant::now(),
        );
    }

    pub(crate) fn stats_view(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let p = palette(self.dark);
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.page = super::Page::Main;
        }

        ui.horizontal(|ui| {
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
                    self.rate_history.clear();
                    self.last_sample = None;
                    self.last_totals = (0, 0);
                    self.notify(("统计已清零".into(), GREEN));
                }
            });
        });
        ui.add_space(6.0);
        self.error_banner(ui);
        let timeline = self.engine.timeline_snapshot();

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
                self.heatmap_card(ui, &p);
                ui.add_space(8.0);
                for i in 0..self.cfg.upstreams.len() {
                    let cells = timeline.get(i).map(|v| v.as_slice()).unwrap_or(&[]);
                    self.upstream_stats_card(ui, snap, i, cells, &p);
                    ui.add_space(8.0);
                }
                if self.cfg.upstreams.is_empty() {
                    ui.label(
                        egui::RichText::new("未配置上游，请求将全部直连")
                            .size(SZ_SMALL)
                            .color(p.weak),
                    );
                }
            });
    }

    /// 汇总条卡：卡头运行状态与当前上游，下方 4 列大数字（对标仪表盘活跃度条）。
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
            ui.add_space(6.0);
            big_stat_row(
                ui,
                p,
                &[
                    (&fmt_bytes(t_up), "累计上传"),
                    (&fmt_bytes(t_down), "累计下载"),
                    (&t_conns.to_string(), "连接总数"),
                    (&format!("{}/s", fmt_bytes(r_up + r_down)), "当前速率 ↑+↓"),
                ],
            );
        });
    }

    /// 流量活动热力图卡：每秒一格的全局总速率（↑+↓），最近 15 分钟。
    fn heatmap_card(&mut self, ui: &mut egui::Ui, p: &super::theme::Palette) {
        card(ui, p.card, p.card_stroke, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("流量活动").strong().size(SZ_SECTION));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new("最近 15 分钟 · 每格 1 秒")
                            .size(SZ_TINY)
                            .color(p.weak),
                    );
                });
            });
            ui.add_space(4.0);
            self.heatmap(ui);
        });
    }

    /// 健康检查时间线：120 格 × 10s（最近 20 分钟），最新在右。
    ///
    /// 绿=全成功、红=全失败、黄=成败混合、弱底色=无样本；悬停显示该格探测明细。
    /// 格宽自适应可用宽度（下限 ~2px），与热力图同一套绘制手法。
    fn health_timeline(
        &mut self,
        ui: &mut egui::Ui,
        cells: &[crate::engine::TimelineCell],
        p: &super::theme::Palette,
    ) {
        use samples::hover_lines;
        let n = cells.len().max(1);
        let gap = 1.0;
        let cell = ((ui.available_width() - gap * (n as f32 - 1.0)) / n as f32).max(2.0);
        let size = egui::vec2(n as f32 * (cell + gap) - gap, 12.0);
        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
        let painter = ui.painter_at(rect);
        // 悬停格索引（列定位与 heatmap 同款），循环后一次性设置悬浮文本
        let hover = resp
            .hover_pos()
            .filter(|h| rect.contains(*h))
            .map(|h| ((h.x - rect.left()) / (cell + gap)) as usize);
        for (k, c) in cells.iter().enumerate() {
            let min = egui::pos2(rect.left() + k as f32 * (cell + gap), rect.top());
            painter.rect_filled(
                egui::Rect::from_min_size(min, egui::vec2(cell, 12.0)),
                1.0,
                cell_color(&c.kind, p),
            );
        }
        if let Some(k) = hover
            && let Some(c) = cells.get(k)
            && let Some(text) = hover_lines(&c.samples)
        {
            resp.on_hover_text(text);
        }
    }

    /// GitHub 风格热力图：60 格/行 × 15 行，悬停显示时刻与速率。
    ///
    /// 格子宽度自适应可用宽度（下限 ~3px；更窄的窗口接受裁剪，不引入滚动）。
    /// 颜色以历史峰值每帧重算：峰值创新高时整图平滑重缩放。
    fn heatmap(&mut self, ui: &mut egui::Ui) {
        use samples::{HEAT_COLS, SAMPLE_CAP, heat_level};
        let rows = SAMPLE_CAP / HEAT_COLS;
        let gap = 2.0;
        let cell =
            ((ui.available_width() - gap * (HEAT_COLS as f32 - 1.0)) / HEAT_COLS as f32).max(3.0);
        let size = egui::vec2(
            HEAT_COLS as f32 * (cell + gap) - gap,
            rows as f32 * (cell + gap) - gap,
        );
        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
        let painter = ui.painter_at(rect);
        let scale = crate::ui::theme::heat_scale(self.dark);
        let peak = self.rate_history.iter().map(|s| s.rate).max().unwrap_or(0);
        for i in 0..SAMPLE_CAP {
            let (col, row) = (i % HEAT_COLS, i / HEAT_COLS);
            let min = egui::pos2(
                rect.left() + col as f32 * (cell + gap),
                rect.top() + row as f32 * (cell + gap),
            );
            let color = match self.rate_history.get(i) {
                Some(s) => scale[heat_level(s.rate, peak) as usize],
                None => scale[0],
            };
            painter.rect_filled(
                egui::Rect::from_min_size(min, egui::vec2(cell, cell)),
                2.0,
                color,
            );
        }
        if let Some(hover) = resp.hover_pos().filter(|h| rect.contains(*h)) {
            let col = ((hover.x - rect.left()) / (cell + gap)) as usize;
            let row = ((hover.y - rect.top()) / (cell + gap)) as usize;
            if let Some(s) = self.rate_history.get(row * HEAT_COLS + col) {
                resp.show_tooltip_text(format!(
                    "{} · {}/s",
                    s.at.format("%H:%M:%S"),
                    fmt_bytes(s.rate)
                ));
            }
        }
    }

    /// 单上游统计卡：健康（状态/延迟/检查计数/时间线）、累计流量、速率与峰值。
    fn upstream_stats_card(
        &mut self,
        ui: &mut egui::Ui,
        snap: &Snapshot,
        i: usize,
        cells: &[crate::engine::TimelineCell],
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
            ui.add_space(6.0);
            if let Some(u) = st {
                // 中部：3 列大数字
                big_stat_row(
                    ui,
                    p,
                    &[
                        (&fmt_bytes(u.bytes_up), "累计上传"),
                        (&fmt_bytes(u.bytes_down), "累计下载"),
                        (&u.conns.to_string(), "连接次数"),
                    ],
                );
                ui.add_space(4.0);
                // 速率条：满量程取上/下行峰值之和（两峰值未必同时发生，
                // 和值 ≥ 任意真实瞬间总速率，达不到满格属预期，偏保守）
                let cur = u.rate_up + u.rate_down;
                let peak = u.peak_up + u.peak_down;
                let frac = if peak > 0 {
                    cur as f32 / peak as f32
                } else {
                    0.0
                };
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("速率").size(SZ_SMALL).color(p.weak));
                    rate_bar(ui, frac, GREEN, p.field);
                });
                let peak_at = u.peak_at.as_deref().unwrap_or("—");
                ui.label(
                    egui::RichText::new(format!(
                        "当前 ↑ {}/s  ↓ {}/s  ·  峰值 ↑ {}/s  ↓ {}/s（{peak_at}）",
                        fmt_bytes(u.rate_up),
                        fmt_bytes(u.rate_down),
                        fmt_bytes(u.peak_up),
                        fmt_bytes(u.peak_down),
                    ))
                    .size(SZ_TINY)
                    .color(p.weak),
                );
                ui.add_space(2.0);
                // 健康时间线：最近 20 分钟逐格探测结果
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("健康").size(SZ_SMALL).color(p.weak));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new("最近 20 分钟 · 每格 10 秒")
                                .size(SZ_TINY)
                                .color(p.weak),
                        );
                    });
                });
                self.health_timeline(ui, cells, p);
                ui.add_space(2.0);
                // 健康尾行
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
                ui.label(egui::RichText::new(health).size(SZ_SMALL).color(p.weak));
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

/// 大数字行：n 列均分可用宽度，列间自绘 1px 竖分隔线。
///
/// 行内 `item_spacing` 归零、列宽按「可用宽 − 分隔线」精确计算：
/// egui 默认元素间距（8px/项）与分隔线固有宽度不占公式，否则整行溢出卡片。
fn big_stat_row(ui: &mut egui::Ui, p: &super::theme::Palette, cols: &[(&str, &str)]) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let n = cols.len();
        let sep_w = 1.0;
        let w = (ui.available_width() - sep_w * (n as f32 - 1.0)) / n as f32;
        for (i, (value, label)) in cols.iter().enumerate() {
            if i > 0 {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(sep_w, 30.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 0.0, p.card_stroke);
            }
            big_stat_col(ui, p, value, label, w);
        }
    });
}

/// 大数字单元格：定宽居中的 SZ_NUM 粗体数值 + 弱色小标签。
fn big_stat_col(
    ui: &mut egui::Ui,
    p: &super::theme::Palette,
    value: &str,
    label: &str,
    width: f32,
) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 44.0),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            ui.label(egui::RichText::new(value).strong().size(SZ_NUM));
            ui.label(egui::RichText::new(label).size(SZ_TINY).color(p.weak));
        },
    );
}

#[cfg(test)]
mod tests {
    use super::samples::{SAMPLE_CAP, Sample, heat_level, push_sample};
    use std::collections::VecDeque;
    use std::time::{Duration, Instant};

    mod sparkline {
        use crate::ui::stats::samples::{Sample, sparkline_points};
        use std::collections::VecDeque;

        fn sample(rate: u64) -> Sample {
            Sample {
                at: chrono::Local::now(),
                rate,
            }
        }

        #[test]
        fn empty_history_yields_no_points() {
            assert!(sparkline_points(&VecDeque::new(), 30).is_empty());
        }

        #[test]
        fn points_normalize_by_peak_bottom_to_top() {
            let h: VecDeque<_> = [0u64, 100, 200, 400].iter().map(|&r| sample(r)).collect();
            let pts = sparkline_points(&h, 4);
            assert_eq!(pts.len(), 4);
            // 零速率贴底（y=1），峰值触顶（y=0）
            assert!((pts[0].1 - 1.0).abs() < 1e-6);
            assert!((pts[3].1 - 0.0).abs() < 1e-6);
            // x 单调递增覆盖 0..1
            assert!((pts[0].0).abs() < 1e-6 && (pts[3].0 - 1.0).abs() < 1e-6);
        }

        #[test]
        fn flat_zero_history_sits_at_bottom() {
            let h: VecDeque<_> = [0u64, 0, 0].iter().map(|&r| sample(r)).collect();
            assert!(
                sparkline_points(&h, 3)
                    .iter()
                    .all(|(_, y)| (*y - 1.0).abs() < 1e-6)
            );
        }

        #[test]
        fn takes_only_the_last_n_samples() {
            let h: VecDeque<_> = (0..10u64).map(|r| sample(r * 10)).collect();
            let pts = sparkline_points(&h, 4);
            assert_eq!(pts.len(), 4);
            // 截取尾部：首点是全局第 6 个样本（rate=60），非峰值（rate=90）→ 未触顶
            assert!(pts[0].1 > 0.0);
            assert!((pts[3].1 - 0.0).abs() < 1e-6);
        }
    }

    mod timeline_ui {
        use crate::engine::{CellKind, CheckSample, TimelineCell};
        use crate::ui::stats::{cell_color, samples::hover_lines};
        use crate::ui::theme::{GREEN, RED, YELLOW, palette};
        use std::time::Instant;

        fn s(stamp: &str, ms: Option<u64>) -> CheckSample {
            CheckSample {
                at: Instant::now(),
                stamp: stamp.into(),
                latency_ms: ms,
            }
        }

        #[test]
        fn hover_lines_format_samples_or_none() {
            assert_eq!(hover_lines(&[]), None);
            let cell = TimelineCell {
                kind: CellKind::Mixed,
                samples: vec![s("12:00:01", Some(311)), s("12:00:05", None)],
            };
            assert_eq!(
                hover_lines(&cell.samples).unwrap(),
                "12:00:01 ✓ 311 ms\n12:00:05 ✗"
            );
        }

        #[test]
        fn cell_colors_follow_kind() {
            let p = palette(true);
            assert_eq!(cell_color(&CellKind::AllOk, &p), GREEN);
            assert_eq!(cell_color(&CellKind::AllFail, &p), RED);
            assert_eq!(cell_color(&CellKind::Mixed, &p), YELLOW);
            assert_eq!(cell_color(&CellKind::Empty, &p), p.field);
        }
    }

    fn base() -> (VecDeque<Sample>, Option<Instant>, (u64, u64)) {
        (VecDeque::new(), None, (0, 0))
    }

    #[test]
    fn first_sample_pushes_immediately() {
        let (mut h, mut last, mut totals) = base();
        let now = Instant::now();
        assert!(push_sample(
            &mut h,
            &mut last,
            &mut totals,
            (10, 20),
            30,
            now
        ));
        assert_eq!(h.len(), 1);
        assert_eq!(h.back().unwrap().rate, 30);
        assert_eq!(totals, (10, 20));
    }

    #[test]
    fn throttles_samples_within_one_second() {
        let (mut h, mut last, mut totals) = base();
        let now = Instant::now();
        assert!(push_sample(
            &mut h,
            &mut last,
            &mut totals,
            (10, 0),
            10,
            now
        ));
        let n = h.len();
        // 距上次入样不足 1s：不推入、基准不前进
        assert!(!push_sample(
            &mut h,
            &mut last,
            &mut totals,
            (99, 99),
            0,
            now + Duration::from_millis(500)
        ));
        assert_eq!(h.len(), n);
        assert_eq!(totals, (10, 0));
        // 满 1s：推入
        assert!(push_sample(
            &mut h,
            &mut last,
            &mut totals,
            (15, 0),
            15,
            now + Duration::from_secs(1)
        ));
        assert_eq!(h.len(), n + 1);
    }

    #[test]
    fn counter_drop_clears_history_and_rebases() {
        let (mut h, mut last, mut totals) = base();
        let now = Instant::now();
        let t0 = now;
        for i in 0..5u64 {
            push_sample(
                &mut h,
                &mut last,
                &mut totals,
                (100 + i * 10, 0),
                10 + i,
                t0 + Duration::from_secs(i),
            );
        }
        assert_eq!(h.len(), 5);
        // 引擎重启/清零：计数回落 → 历史清空、基准重置、节流重置
        assert!(!push_sample(
            &mut h,
            &mut last,
            &mut totals,
            (7, 3),
            10,
            t0 + Duration::from_secs(5)
        ));
        assert!(h.is_empty());
        assert_eq!(totals, (7, 3));
        assert_eq!(last, None);
        // 回落当帧不推样；下一秒起正常重新积累
        assert!(push_sample(
            &mut h,
            &mut last,
            &mut totals,
            (9, 5),
            14,
            t0 + Duration::from_secs(6)
        ));
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn drops_oldest_when_full() {
        let (mut h, mut last, mut totals) = base();
        let now = Instant::now();
        for i in 0..SAMPLE_CAP as u64 {
            push_sample(
                &mut h,
                &mut last,
                &mut totals,
                (i, 0),
                i,
                now + Duration::from_secs(i),
            );
        }
        assert_eq!(h.len(), SAMPLE_CAP);
        let oldest = h.front().unwrap().rate;
        assert!(push_sample(
            &mut h,
            &mut last,
            &mut totals,
            (u64::MAX, 0),
            u64::MAX,
            now + Duration::from_secs(SAMPLE_CAP as u64)
        ));
        assert_eq!(h.len(), SAMPLE_CAP);
        assert_ne!(h.front().unwrap().rate, oldest);
        assert_eq!(h.back().unwrap().rate, u64::MAX);
    }

    #[test]
    fn heat_levels() {
        // 无流量或无参考峰值 → 0 档
        assert_eq!(heat_level(0, 100), 0);
        assert_eq!(heat_level(0, 0), 0);
        assert_eq!(heat_level(100, 0), 0);
        // 按比例分档（≤25/50/75/100%）
        assert_eq!(heat_level(25, 100), 1);
        assert_eq!(heat_level(26, 100), 2);
        assert_eq!(heat_level(50, 100), 2);
        assert_eq!(heat_level(51, 100), 3);
        assert_eq!(heat_level(75, 100), 3);
        assert_eq!(heat_level(76, 100), 4);
        assert_eq!(heat_level(100, 100), 4);
    }
}
