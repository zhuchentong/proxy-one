//! 侧边导航栏：页面切换、主题切换与 painter 手绘矢量图标。
//!
//! 图标全部用 `egui::Painter` 基元绘制（不引图标字体/字符字形），
//! 主题感知、高分屏清晰；设置页图标用「滑块」造型，比齿轮更现代。

use eframe::egui;

use super::theme::{GREEN, palette};
use super::{App, Page};

/// 52px 窄条侧栏：品牌点 + 导航 + 底部主题切换。
pub(crate) fn side_panel(app: &mut App, ui: &mut egui::Ui) {
    egui::Panel::left("nav")
        .exact_size(52.0)
        .resizable(false)
        .frame(egui::Frame::new().fill(palette(app.dark).rail))
        .show(ui, |ui| {
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                ui.add_space(12.0);
                // 品牌圆点（logo）
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(36.0, 22.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 5.0, GREEN);
                ui.add_space(14.0);
                for (page, tip) in Page::NAV {
                    if nav_button(ui, app.page == page, tip, app.dark, |p, r, c| {
                        draw_page_icon(p, r, page, c)
                    })
                    .clicked()
                    {
                        app.page = page;
                    }
                }
                // 底部主题切换：bottom_up 布局吃掉剩余空间，把按钮推到底
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    ui.add_space(12.0);
                    let tip = if app.dark {
                        "切换到浅色主题"
                    } else {
                        "切换到深色主题"
                    };
                    if nav_button(ui, false, tip, app.dark, draw_theme_icon).clicked() {
                        app.toggle_theme(ui.ctx());
                    }
                });
            });
        });
}

/// 方形导航钮：36×36 命中区域，选中/悬停有圆角底，图标按状态着色。
fn nav_button(
    ui: &mut egui::Ui,
    selected: bool,
    tooltip: &str,
    dark: bool,
    draw: impl Fn(&egui::Painter, egui::Rect, egui::Color32),
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::click());
    let p = palette(dark);
    if selected || resp.hovered() {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(8), p.hover);
    }
    let color = if selected {
        GREEN
    } else if resp.hovered() {
        p.text
    } else {
        p.weak
    };
    draw(ui.painter(), rect, color);
    if tooltip.is_empty() {
        resp
    } else {
        resp.on_hover_text(tooltip)
    }
}

fn draw_page_icon(p: &egui::Painter, r: egui::Rect, page: Page, c: egui::Color32) {
    match page {
        Page::Main => draw_home(p, r, c),
        Page::Stats => draw_stats(p, r, c),
        Page::Logs => draw_logs(p, r, c),
        Page::Settings => draw_settings(p, r, c),
    }
}

/// 主页：屋顶三角 + 屋身方框。
fn draw_home(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let (x, y) = (r.center().x, r.center().y);
    let roof = [
        egui::pos2(x - 9.0, y - 1.0),
        egui::pos2(x, y - 9.0),
        egui::pos2(x + 9.0, y - 1.0),
    ];
    p.add(egui::Shape::line(roof.to_vec(), egui::Stroke::new(1.6, c)));
    p.rect_stroke(
        egui::Rect::from_min_max(egui::pos2(x - 6.0, y - 1.0), egui::pos2(x + 6.0, y + 8.0)),
        1.0,
        egui::Stroke::new(1.6, c),
        egui::StrokeKind::Inside,
    );
}

/// 统计：三根逐级升高的实心柱。
fn draw_stats(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let cx = r.center().x;
    let base = r.center().y + 8.0;
    for (i, h) in [7.0f32, 11.5, 16.0].iter().enumerate() {
        let x = cx - 7.0 + i as f32 * 5.0;
        p.rect_filled(
            egui::Rect::from_min_size(egui::pos2(x, base - h), egui::vec2(3.5, *h)),
            1.0,
            c,
        );
    }
}

/// 日志：文档 = 三条横线（末行收窄）。
fn draw_logs(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let (cx, cy) = (r.center().x, r.center().y);
    let stroke = egui::Stroke::new(1.6, c);
    for (i, w) in [16.0f32, 16.0, 10.0].iter().enumerate() {
        let y = cy - 6.0 + i as f32 * 6.0;
        p.line_segment(
            [egui::pos2(cx - *w / 2.0, y), egui::pos2(cx + *w / 2.0, y)],
            stroke,
        );
    }
}

/// 设置：两条滑轨，各带一个圆点旋钮（滑块造型）。
fn draw_settings(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let (cx, cy) = (r.center().x, r.center().y);
    let stroke = egui::Stroke::new(1.6, c);
    p.line_segment(
        [
            egui::pos2(cx - 8.0, cy - 4.0),
            egui::pos2(cx + 8.0, cy - 4.0),
        ],
        stroke,
    );
    p.circle_filled(egui::pos2(cx + 3.0, cy - 4.0), 2.6, c);
    p.line_segment(
        [
            egui::pos2(cx - 8.0, cy + 4.0),
            egui::pos2(cx + 8.0, cy + 4.0),
        ],
        stroke,
    );
    p.circle_filled(egui::pos2(cx - 3.0, cy + 4.0), 2.6, c);
}

/// 主题：半填充圆（右半实心 + 整圆描边）。
fn draw_theme_icon(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let center = r.center();
    p.circle_stroke(center, 7.5, egui::Stroke::new(1.6, c));
    let right_half = egui::Rect::from_min_max(
        egui::pos2(center.x, center.y - 8.0),
        egui::pos2(center.x + 8.0, center.y + 8.0),
    );
    p.with_clip_rect(right_half).circle_filled(center, 7.5, c);
}
