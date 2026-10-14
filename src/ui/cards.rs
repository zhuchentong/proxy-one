//! 主界面各视图与卡片：状态、上游列表、添加表单、日志与页脚。
//! 独立设置页见 [`super::settings`]。

use eframe::egui;

use super::App;
use super::stats::samples;
use super::theme::{
    GRAY, GREEN, ORANGE, RED, SZ_NUM, SZ_SECTION, SZ_SMALL, SZ_TINY, YELLOW, palette,
};
use super::widgets::{
    ButtonVariant, card, chip, latency_color, phase_indicator, status_glyph, styled_button,
    styled_button_ex,
};
use crate::config::{self, UpstreamConfig, UpstreamKind};
use crate::engine::{HealthStatus, Phase, Snapshot};
use crate::util::fmt_bytes;

impl App {
    pub(crate) fn status_card(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let p = palette(self.dark);
        let (dot, label) = phase_indicator(snap.phase);
        let running = matches!(
            snap.phase,
            Phase::Running | Phase::Starting | Phase::Stopping
        );
        card(ui, p.card, p.card_stroke, |ui| {
            ui.set_min_width(ui.available_width());
            // 行 1：大状态 + 启停
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("●").color(dot).size(17.0));
                ui.label(egui::RichText::new(label).strong().size(SZ_NUM));
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
            // 行 2：实时速率大数字 + 迷你速率图（复用统计页每秒采样）
            let (tot_up, tot_down, tot_conns, tot_rup, tot_rdown) = snap.traffic_totals();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("↑").size(SZ_NUM).color(p.weak));
                ui.label(
                    egui::RichText::new(format!("{}/s", fmt_bytes(tot_rup)))
                        .monospace()
                        .size(SZ_NUM),
                );
                ui.add_space(12.0);
                ui.label(egui::RichText::new("↓").size(SZ_NUM).color(p.weak));
                ui.label(
                    egui::RichText::new(format!("{}/s", fmt_bytes(tot_rdown)))
                        .monospace()
                        .size(SZ_NUM),
                );
                let w = (ui.available_width() - 4.0).clamp(40.0, 160.0);
                draw_sparkline(
                    ui,
                    &self.rate_history,
                    egui::vec2(w, 26.0),
                    GREEN.gamma_multiply(0.85),
                    p.field,
                );
            });
            // 行 3：监听地址 + 当前上游 + 通知消息
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
            // 行 4（可选）：累计转发流量与连接数（有活动才显示）
            if tot_conns > 0 || tot_up > 0 || tot_down > 0 {
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(format!(
                        "累计 ↑ {}  ↓ {}  ·  连接 {}",
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
        let testing = st.is_some_and(|u| u.testing);
        let accent = row_accent(st.map(|u| u.status), testing);
        let stroke = if is_active { GREEN } else { p.card_stroke };
        let editing = self.row_edit.as_ref().is_some_and(|e| e.idx == i);

        let mut begin = false;
        let mut confirm = false;
        let mut cancel = false;
        let resp = card(ui, p.card, stroke, |ui| {
            ui.set_min_width(ui.available_width());
            // 行高不因 hover 抖动：操作钮常驻占位，非 hover 时着色弱化到近隐形
            let hovered = ui.rect_contains_pointer(ui.max_rect());
            let action = if hovered || editing {
                p.text
            } else {
                p.card_stroke
            };
            if editing {
                let Some(edit) = &mut self.row_edit else {
                    return;
                };
                let pri = self.cfg.upstreams[i].priority;
                ui.horizontal(|ui| {
                    chip(ui, &format!("P{pri}"), GREEN, p.chip);
                    ui.add(
                        egui::TextEdit::singleline(&mut edit.draft_mut().name)
                            .desired_width(70.0)
                            .font(egui::TextStyle::Body),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut edit.draft_mut().addr)
                            .desired_width(ui.available_width() - 220.0)
                            .font(egui::TextStyle::Monospace),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if row_icon_button(ui, "×", "取消编辑", RED).clicked() {
                            cancel = true;
                        }
                        if row_icon_button(ui, "✓", "确认修改", GREEN).clicked() {
                            confirm = true;
                        }
                        let before = edit.draft().kind;
                        egui::ComboBox::from_id_salt(format!("edit-kind{i}"))
                            .selected_text(edit.draft().kind.label())
                            .width(58.0)
                            .show_ui(ui, |ui| {
                                for k in
                                    [UpstreamKind::Auto, UpstreamKind::Http, UpstreamKind::Socks5]
                                {
                                    ui.selectable_value(&mut edit.draft_mut().kind, k, k.label());
                                }
                            });
                        let _ = before;
                    });
                });
            } else {
                let pri = self.cfg.upstreams[i].priority;
                let name = self.cfg.upstreams[i].name.clone();
                let kind_label = self.cfg.upstreams[i].kind.label();
                ui.horizontal(|ui| {
                    chip(ui, &format!("P{pri}"), GREEN, p.chip);
                    ui.label(egui::RichText::new(&name).strong().size(SZ_SECTION));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if row_icon_button(ui, "×", "删除（保存后生效）", action).clicked()
                        {
                            self.pending_action = Some(RowAction::Del(i));
                        }
                        if row_icon_button(ui, "↓", "下移", action).clicked() {
                            self.pending_action = Some(RowAction::Down(i));
                        }
                        if row_icon_button(ui, "↑", "上移", action).clicked() {
                            self.pending_action = Some(RowAction::Up(i));
                        }
                        if row_icon_button(ui, "✎", "编辑名称/地址/类型", action).clicked()
                        {
                            begin = true;
                        }
                        // 健康状态与延迟（与旧行一致）
                        if testing {
                            ui.add(egui::Spinner::new().size(13.0).color(p.text));
                            ui.label(egui::RichText::new("测试中").size(SZ_SMALL).color(p.weak));
                        } else {
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
                                    ui.label(
                                        egui::RichText::new("— ms").size(SZ_SMALL).color(p.weak),
                                    );
                                }
                            };
                        }
                        match st {
                            Some(u) => {
                                let (d, c) = status_glyph(u.status);
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
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(&self.cfg.upstreams[i].addr)
                            .monospace()
                            .size(SZ_SMALL)
                            .color(p.weak),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(kind_label).size(SZ_TINY).color(p.weak));
                    });
                });
            }
            // 累计转发流量；有活动窗口时前缀当前速率（1s 采样）
            if let Some(u) = st.filter(|u| u.conns > 0 || u.rate_up > 0 || u.rate_down > 0) {
                ui.add_space(2.0);
                let rate = if u.rate_up > 0 || u.rate_down > 0 {
                    format!(
                        "↑ {}/s  ↓ {}/s  ·  ",
                        fmt_bytes(u.rate_up),
                        fmt_bytes(u.rate_down)
                    )
                } else {
                    String::new()
                };
                ui.label(
                    egui::RichText::new(format!(
                        "{rate}↑ {}  ↓ {}  ·  {} 次连接",
                        fmt_bytes(u.bytes_up),
                        fmt_bytes(u.bytes_down),
                        u.conns
                    ))
                    .size(SZ_TINY)
                    .color(p.weak),
                );
            }
        });
        // 左侧状态色条：贴卡片左缘、避开圆角的竖条
        let r = resp.response.rect;
        let bar = egui::Rect::from_min_max(
            r.left_top() + egui::vec2(0.0, 7.0),
            r.left_bottom() + egui::vec2(3.0, -7.0),
        );
        ui.painter_at(r).rect_filled(bar, 1.0, accent);

        if begin {
            self.begin_row_edit(i);
        }
        if confirm {
            self.confirm_row_edit();
        }
        if cancel {
            self.cancel_row_edit();
        }
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

    /// 进入行编辑：快照该行原值为草稿（同刻关闭其它行的编辑）。
    pub(crate) fn begin_row_edit(&mut self, i: usize) {
        if let Some(u) = self.cfg.upstreams.get(i) {
            self.row_edit = Some(RowEdit::begin(i, u));
        }
    }

    /// 确认编辑：草稿写回配置并标记未保存；行号失效时静默丢弃。
    pub(crate) fn confirm_row_edit(&mut self) {
        if let Some(e) = self.row_edit.take() {
            let (i, draft) = e.confirm();
            if let Some(slot) = self.cfg.upstreams.get_mut(i) {
                *slot = draft;
                self.dirty = true;
            }
        }
    }

    /// 取消编辑：直接丢弃草稿。
    pub(crate) fn cancel_row_edit(&mut self) {
        self.row_edit = None;
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

/// 行内幽灵图标钮：透明底、按传入色着色（hover 排用弱化色避免行高抖动）。
fn row_icon_button(
    ui: &mut egui::Ui,
    glyph: &str,
    tooltip: &str,
    color: egui::Color32,
) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(glyph).size(12.0).color(color))
            .fill(egui::Color32::TRANSPARENT)
            .min_size(egui::vec2(22.0, 22.0)),
    )
    .on_hover_text(tooltip)
}

/// Hero 迷你速率图：底部场色 + 绿色折线（取最近 60 个每秒采样）。
fn draw_sparkline(
    ui: &mut egui::Ui,
    history: &std::collections::VecDeque<samples::Sample>,
    size: egui::Vec2,
    line: egui::Color32,
    bg: egui::Color32,
) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, bg);
    let pts = samples::sparkline_points(history, 60);
    if pts.len() >= 2 {
        let points: Vec<egui::Pos2> = pts
            .iter()
            .map(|(x, y)| {
                egui::pos2(
                    rect.left() + 2.0 + x * (rect.width() - 4.0),
                    rect.top() + 2.0 + y * (rect.height() - 4.0),
                )
            })
            .collect();
        painter.add(egui::Shape::line(points, egui::Stroke::new(1.5, line)));
    }
}

/// 上游行编辑状态：进入时快照原值为草稿，编辑只改草稿；
/// 确认时消费自身返回 `(行号, 草稿)` 供写回，取消直接丢弃。
pub(crate) struct RowEdit {
    pub(crate) idx: usize,
    draft: UpstreamConfig,
}

impl RowEdit {
    pub(crate) fn begin(idx: usize, original: &UpstreamConfig) -> Self {
        RowEdit {
            idx,
            draft: original.clone(),
        }
    }

    pub(crate) fn draft(&self) -> &UpstreamConfig {
        &self.draft
    }

    pub(crate) fn draft_mut(&mut self) -> &mut UpstreamConfig {
        &mut self.draft
    }

    pub(crate) fn confirm(self) -> (usize, UpstreamConfig) {
        (self.idx, self.draft)
    }
}

/// 行首状态色条：绿=健康、红=故障、灰=未知/无数据，测试中一律黄色提示。
fn row_accent(status: Option<HealthStatus>, testing: bool) -> egui::Color32 {
    if testing {
        return YELLOW;
    }
    match status {
        Some(HealthStatus::Up) => GREEN,
        Some(HealthStatus::Down) => RED,
        Some(HealthStatus::Unknown) | None => GRAY,
    }
}

#[derive(Clone, Copy)]
pub(crate) enum RowAction {
    Up(usize),
    Down(usize),
    Del(usize),
}

#[cfg(test)]
mod tests {
    use super::super::theme::{GRAY, GREEN, RED, YELLOW};
    use super::{RowEdit, row_accent};
    use crate::config::UpstreamConfig;
    use crate::engine::HealthStatus;

    fn sample() -> UpstreamConfig {
        UpstreamConfig {
            name: "a".into(),
            addr: "host:1".into(),
            kind: Default::default(),
            priority: 1,
            username: None,
            password: None,
        }
    }

    #[test]
    fn draft_edits_do_not_touch_original_until_confirm() {
        let original = sample();
        let mut edit = RowEdit::begin(0, &original);
        edit.draft_mut().name = "b".into();
        // 编辑期间原值不动；确认时才返回草稿供写回
        assert_eq!(original.name, "a");
        let (idx, draft) = edit.confirm();
        assert_eq!(idx, 0);
        assert_eq!(draft.name, "b");
        assert_eq!(draft.addr, "host:1");
    }

    #[test]
    fn accent_follows_status_with_testing_highest_priority() {
        assert_eq!(row_accent(Some(HealthStatus::Up), false), GREEN);
        assert_eq!(row_accent(Some(HealthStatus::Down), false), RED);
        assert_eq!(row_accent(Some(HealthStatus::Unknown), false), GRAY);
        assert_eq!(row_accent(None, false), GRAY);
        // 测试中无论健康与否都用黄色提示
        assert_eq!(row_accent(Some(HealthStatus::Up), true), YELLOW);
        assert_eq!(row_accent(Some(HealthStatus::Down), true), YELLOW);
    }
}
