//! 可复用的极简 UI 组件：卡片、胶囊徽章、拨动开关、按钮系统与日志着色。

use eframe::egui;

use super::theme::{GRAY, GREEN, RED, SZ_TINY, YELLOW, palette};
use crate::engine::{HealthStatus, LogEntry, LogLevel, Phase};

/// 圆角卡片容器，`stroke` 用于强调（如「使用中」的上游卡片为品牌绿）。
/// 返回响应以便调用方在卡片矩形上叠加绘制（如状态色条）。
pub(crate) fn card<R>(
    ui: &mut egui::Ui,
    fill: egui::Color32,
    stroke: egui::Color32,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(egui::CornerRadius::same(10))
        .stroke(egui::Stroke::new(1.0, stroke))
        .inner_margin(egui::Margin::same(10))
        .show(ui, add)
}

/// 小号胶囊徽章（优先级、当前上游等）。
pub(crate) fn chip(ui: &mut egui::Ui, text: &str, fg: egui::Color32, bg: egui::Color32) {
    egui::Frame::new()
        .fill(bg)
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(5, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(10.5).color(fg).strong());
        });
}

/// 按钮语义分级：Primary=主操作（启动/保存）、Danger=破坏性（停止）、
/// Ghost=次要操作（取消/添加入口）。
#[derive(Clone, Copy)]
pub(crate) enum ButtonVariant {
    Primary,
    Danger,
    Ghost,
}

/// 统一文字按钮：高度 26、圆角 8，按主题与语义配色，悬停反馈由 egui 提供。
pub(crate) fn styled_button(
    ui: &mut egui::Ui,
    text: &str,
    variant: ButtonVariant,
    dark: bool,
) -> egui::Response {
    styled_button_ex(ui, text, variant, dark, 26.0)
}

/// 指定高度的变体（与 24px 图标钮同行时使用 24）。
pub(crate) fn styled_button_ex(
    ui: &mut egui::Ui,
    text: &str,
    variant: ButtonVariant,
    dark: bool,
    height: f32,
) -> egui::Response {
    let p = palette(dark);
    let (fill, text_color, stroke) = match variant {
        ButtonVariant::Primary => (GREEN, egui::Color32::WHITE, egui::Stroke::NONE),
        ButtonVariant::Danger => (
            RED.gamma_multiply(0.16),
            RED,
            egui::Stroke::new(1.0, RED.gamma_multiply(0.55)),
        ),
        ButtonVariant::Ghost => (
            egui::Color32::TRANSPARENT,
            p.text,
            egui::Stroke::new(1.0, p.card_stroke),
        ),
    };
    let font_size = if height >= 26.0 { 12.5 } else { 11.0 };
    ui.add(
        egui::Button::new(
            egui::RichText::new(text)
                .strong()
                .size(font_size)
                .color(text_color),
        )
        .fill(fill)
        .stroke(stroke)
        .corner_radius(egui::CornerRadius::same(8))
        .min_size(egui::vec2(0.0, height)),
    )
}

/// 极简拨动开关：36×19 圆角胶囊 + 圆形滑块，点击切换。
pub(crate) fn toggle_switch(ui: &mut egui::Ui, on: &mut bool, dark: bool) -> egui::Response {
    let size = egui::vec2(36.0, 19.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let off = if dark {
            egui::Color32::from_rgb(0x4a, 0x4f, 0x57)
        } else {
            egui::Color32::from_rgb(0xc4, 0xca, 0xd2)
        };
        p.rect_filled(
            rect,
            egui::CornerRadius::same(10),
            if *on { GREEN } else { off },
        );
        let kx = if *on {
            rect.right() - 9.5
        } else {
            rect.left() + 9.5
        };
        p.circle_filled(egui::pos2(kx, rect.center().y), 7.0, egui::Color32::WHITE);
    }
    resp
}

/// 设置项开关行：标题 + 拨动开关 + 弱化说明。返回开关是否被切换。
pub(crate) fn switch_row(
    ui: &mut egui::Ui,
    dark: bool,
    title: &str,
    hint: &str,
    on: &mut bool,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).size(11.5));
        let r = toggle_switch(ui, on, dark);
        changed = r.changed();
        ui.label(
            egui::RichText::new(hint)
                .size(10.5)
                .color(palette(dark).weak),
        );
    });
    changed
}

/// 日志级别配色。
pub(crate) fn level_color(level: LogLevel) -> egui::Color32 {
    match level {
        LogLevel::Info => GRAY,
        LogLevel::Ok => GREEN,
        LogLevel::Warn => YELLOW,
        LogLevel::Error => RED,
    }
}

/// 阶段指示（状态点颜色 + 文案），状态卡与统计页汇总卡共用。
pub(crate) fn phase_indicator(phase: Phase) -> (egui::Color32, &'static str) {
    match phase {
        Phase::Running => (GREEN, "运行中"),
        Phase::Starting => (YELLOW, "启动中…"),
        Phase::Stopping => (YELLOW, "停止中…"),
        Phase::BindFailed => (RED, "端口绑定失败"),
        Phase::Stopped => (GRAY, "已停止"),
    }
}

/// 健康状态指示（圆点字形 + 颜色），上游卡与统计页共用。
pub(crate) fn status_glyph(status: HealthStatus) -> (&'static str, egui::Color32) {
    match status {
        HealthStatus::Up => ("●", GREEN),
        HealthStatus::Down => ("●", RED),
        HealthStatus::Unknown => ("○", GRAY),
    }
}

/// 延迟着色：<800ms 绿、<3000ms 黄、其余红。
pub(crate) fn latency_color(ms: u64) -> egui::Color32 {
    if ms < 800 {
        GREEN
    } else if ms < 3000 {
        YELLOW
    } else {
        RED
    }
}

/// 单条日志行（等宽、级别着色、可选中），日志卡与日志页共用。
pub(crate) fn log_line(ui: &mut egui::Ui, e: &LogEntry) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(e.line())
                .monospace()
                .size(SZ_TINY)
                .color(level_color(e.level)),
        )
        .selectable(true),
    );
}

/// 自绘圆角细进度条（高 6px、圆角 3）：`frac ∈ [0,1]`，超出按满格处理。
///
/// 语义提示：满量程常取「上/下行峰值之和」，两个峰值未必同时发生，
/// 该和值 ≥ 任意真实瞬间总速率，进度条达不到满格属预期（偏保守）。
pub(crate) fn rate_bar(ui: &mut egui::Ui, frac: f32, fg: egui::Color32, bg: egui::Color32) {
    let height = 6.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    ui.painter().rect_filled(rect, 3.0, bg);
    let frac = frac.clamp(0.0, 1.0);
    if frac > 0.0 {
        let mut fill = rect;
        fill.set_width(rect.width() * frac);
        ui.painter().rect_filled(fill, 3.0, fg);
    }
}
