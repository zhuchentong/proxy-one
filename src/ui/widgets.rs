//! 可复用的极简 UI 组件：卡片、胶囊徽章、拨动开关、按钮系统与日志着色。

use eframe::egui;

use super::theme::{GRAY, GREEN, RED, YELLOW, palette};
use crate::engine::LogLevel;

/// 圆角卡片容器，`stroke` 用于强调（如「使用中」的上游卡片为品牌绿）。
pub(crate) fn card(
    ui: &mut egui::Ui,
    fill: egui::Color32,
    stroke: egui::Color32,
    add: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(egui::CornerRadius::same(10))
        .stroke(egui::Stroke::new(1.0, stroke))
        .inner_margin(egui::Margin::same(10))
        .show(ui, add);
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

/// 方形图标按钮：24×24 统一命中区域，字形居中，用于 ↑ ↓ ✕ ⚙ ☀ 等单字符操作。
pub(crate) fn icon_button(
    ui: &mut egui::Ui,
    glyph: &str,
    tooltip: &str,
    dark: bool,
) -> egui::Response {
    let p = palette(dark);
    let resp = ui.add(
        egui::Button::new(egui::RichText::new(glyph).size(12.0).color(p.text))
            .fill(p.field)
            .stroke(egui::Stroke::new(1.0, p.card_stroke))
            .corner_radius(egui::CornerRadius::same(6))
            .min_size(egui::vec2(24.0, 24.0)),
    );
    if tooltip.is_empty() {
        resp
    } else {
        resp.on_hover_text(tooltip)
    }
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

/// 字节数人性化展示（B / KB / MB / GB）。
pub(crate) fn fmt_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let v = n as f64;
    if v >= GB {
        format!("{:.2} GB", v / GB)
    } else if v >= MB {
        format!("{:.1} MB", v / MB)
    } else if v >= KB {
        format!("{:.1} KB", v / KB)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    use super::fmt_bytes;

    #[test]
    fn fmt_bytes_human_readable() {
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(2048), "2.0 KB");
        assert_eq!(fmt_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(fmt_bytes(3 * 1024 * 1024 * 1024 + 7), "3.00 GB");
    }
}
