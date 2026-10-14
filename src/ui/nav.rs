//! 侧边导航栏：页面切换、主题切换与 SVG 矢量图标（Lucide 风格，取自 iconify）。
//!
//! 图标以 SVG 源码嵌入二进制（`include_image!`），由 `egui_extras` 的
//! `SvgImage` loader 按显示尺寸光栅化，高分屏清晰；`Image::tint` 乘法混合
//! 完成主题着色（Lucide 源为 `stroke="currentColor"` → 黑色 → 染任意色）。

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
                    if nav_button(ui, app.page == page, tip, app.dark, page_icon(page)).clicked() {
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
                    if nav_button(ui, false, tip, app.dark, theme_icon()).clicked() {
                        app.toggle_theme(ui.ctx());
                    }
                });
            });
        });
}

/// 页面导航图标（Lucide 线性风格，编译期嵌入二进制）。
fn page_icon(page: Page) -> egui::ImageSource<'static> {
    match page {
        Page::Main => egui::include_image!("../../assets/icons/house.svg"),
        Page::Stats => egui::include_image!("../../assets/icons/chart-column.svg"),
        Page::Logs => egui::include_image!("../../assets/icons/file-text.svg"),
        Page::Settings => egui::include_image!("../../assets/icons/settings.svg"),
    }
}

/// 主题切换图标：半日半月的合成图标。
fn theme_icon() -> egui::ImageSource<'static> {
    egui::include_image!("../../assets/icons/sun-moon.svg")
}

/// 方形导航钮：36×36 命中区域，选中/悬停有圆角底，图标按状态着色。
fn nav_button(
    ui: &mut egui::Ui,
    selected: bool,
    tooltip: &str,
    dark: bool,
    icon: egui::ImageSource,
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
    let image = egui::Image::new(icon)
        .tint(color)
        .fit_to_exact_size(egui::vec2(18.0, 18.0));
    ui.put(rect, image);
    if tooltip.is_empty() {
        resp
    } else {
        resp.on_hover_text(tooltip)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个页面都能解析出一个图标源（路径资源存在性在编译期已保证）。
    #[test]
    fn every_page_has_an_icon_source() {
        for (page, _) in Page::NAV {
            match page_icon(page) {
                egui::ImageSource::Bytes { .. } => {}
                _ => panic!("页面图标应为嵌入字节源"),
            }
        }
        assert!(matches!(theme_icon(), egui::ImageSource::Bytes { .. }));
    }
}
