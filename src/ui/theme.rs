//! 主题：明暗两套配色、egui Visuals 定制与中文字体加载。

use std::sync::Arc;

use eframe::egui;

pub(crate) const GREEN: egui::Color32 = egui::Color32::from_rgb(0x3f, 0xc1, 0x7a);
pub(crate) const YELLOW: egui::Color32 = egui::Color32::from_rgb(0xd8, 0xa0, 0x3a);
pub(crate) const RED: egui::Color32 = egui::Color32::from_rgb(0xe0, 0x5d, 0x5d);
pub(crate) const GRAY: egui::Color32 = egui::Color32::from_rgb(0x96, 0x96, 0x96);
pub(crate) const ORANGE: egui::Color32 = egui::Color32::from_rgb(0xe0, 0x7b, 0x39);

/// 字号刻度：全 UI 只使用这几档，保证节奏一致。
pub(crate) const SZ_TITLE: f32 = 15.0;
pub(crate) const SZ_SECTION: f32 = 12.5;
pub(crate) const SZ_BODY: f32 = 11.5;
pub(crate) const SZ_SMALL: f32 = 11.0;
pub(crate) const SZ_TINY: f32 = 10.5;

/// 控件统一圆角，与按钮（8）、图标钮（6）形成层级。
pub(crate) const RADIUS_CTRL: u8 = 6;

pub(crate) struct Palette {
    pub bg: egui::Color32,
    pub card: egui::Color32,
    pub card_stroke: egui::Color32,
    pub field: egui::Color32,
    pub text: egui::Color32,
    pub weak: egui::Color32,
    pub hover: egui::Color32,
    pub chip: egui::Color32,
}

pub(crate) fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            bg: egui::Color32::from_rgb(0x15, 0x16, 0x1a),
            card: egui::Color32::from_rgb(0x1f, 0x21, 0x26),
            card_stroke: egui::Color32::from_rgb(0x2c, 0x2f, 0x36),
            field: egui::Color32::from_rgb(0x17, 0x18, 0x1d),
            text: egui::Color32::from_rgb(0xe8, 0xea, 0xed),
            weak: egui::Color32::from_rgb(0x8f, 0x96, 0xa0),
            hover: egui::Color32::from_rgb(0x2a, 0x2d, 0x34),
            chip: egui::Color32::from_rgb(0x27, 0x3a, 0x31),
        }
    } else {
        Palette {
            bg: egui::Color32::from_rgb(0xf2, 0xf4, 0xf7),
            card: egui::Color32::WHITE,
            card_stroke: egui::Color32::from_rgb(0xe2, 0xe6, 0xeb),
            field: egui::Color32::from_rgb(0xf6, 0xf7, 0xf9),
            text: egui::Color32::from_rgb(0x1b, 0x1f, 0x24),
            weak: egui::Color32::from_rgb(0x6b, 0x72, 0x80),
            hover: egui::Color32::from_rgb(0xec, 0xef, 0xf3),
            chip: egui::Color32::from_rgb(0xdf, 0xf3, 0xe8),
        }
    }
}

/// 应用明/暗主题（基于 egui 内置 Visuals 微调背景、文本与选中色）。
pub fn apply_theme(ctx: &egui::Context, dark: bool) {
    let p = palette(dark);
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.panel_fill = p.bg;
    v.window_fill = p.bg;
    v.extreme_bg_color = p.field;
    v.text_edit_bg_color = Some(p.field);
    v.code_bg_color = p.field;
    v.override_text_color = Some(p.text);
    v.widgets.noninteractive.bg_fill = p.card;
    v.widgets.noninteractive.weak_bg_fill = p.card;
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, p.text);
    v.widgets.inactive.weak_bg_fill = p.hover;
    v.widgets.inactive.bg_fill = p.hover;
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, p.text);
    v.widgets.hovered.weak_bg_fill = p.hover;
    v.widgets.hovered.bg_fill = p.hover;
    v.widgets.active.weak_bg_fill = p.hover;
    v.widgets.active.bg_fill = p.hover;
    v.selection.bg_fill = GREEN.gamma_multiply(0.35);
    v.selection.stroke = egui::Stroke::new(1.0, GREEN);
    // 统一控件圆角与细滚动条
    v.widgets.noninteractive.corner_radius = egui::CornerRadius::same(RADIUS_CTRL);
    v.widgets.inactive.corner_radius = egui::CornerRadius::same(RADIUS_CTRL);
    v.widgets.hovered.corner_radius = egui::CornerRadius::same(RADIUS_CTRL);
    v.widgets.active.corner_radius = egui::CornerRadius::same(RADIUS_CTRL);
    ctx.set_visuals(v);
    ctx.all_styles_mut(|style| {
        style.spacing.scroll.bar_width = 6.0;
        style.spacing.scroll.bar_inner_margin = 4.0;
        style.spacing.scroll.bar_outer_margin = 2.0;
    });
}

/// 加载中文字体（按平台探测常见 CJK 字体路径），避免默认字体下的豆腐块。
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    // 跨平台探测链：Windows（雅黑/黑体/宋体）→ Linux（Noto CJK/文泉驿，Manjaro 装
    // noto-fonts-cjk 即命中）。不存在的路径读取失败自动跳过。
    let candidates = [
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\simhei.ttf",
        r"C:\Windows\Fonts\simsun.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJKsc-Regular.otf",
        "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
    ];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                "cjk".to_owned(),
                Arc::new(egui::FontData::from_owned(bytes)),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .push("cjk".to_owned());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("cjk".to_owned());
            break;
        }
    }
    ctx.set_fonts(fonts);
}
