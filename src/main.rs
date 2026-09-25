#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! 代理故障切换网关：混合 HTTP/SOCKS5 入口 + 多上游优先级故障切换 + egui GUI。

mod config;
mod engine;
mod httpc;
mod platform;
mod ui;
mod update;
mod util;

use config::LoadedConfig;

fn main() -> eframe::Result<()> {
    config::migrate_legacy_data_dir();
    let headless = std::env::args().any(|a| a == "--headless");
    let minimized = std::env::args().any(|a| a == "--minimized");
    let updated = std::env::args().any(|a| a == "--updated");
    if updated {
        // 自动更新交接：等旧进程退出并释放监听端口后再绑定
        std::thread::sleep(std::time::Duration::from_millis(1200));
    }
    let loaded = config::load_or_create();
    let dark = loaded.config.general.theme != "light";

    if headless {
        run_headless(loaded.config);
        return Ok(());
    }
    run_gui(loaded, dark, minimized)
}

/// 无界面常驻模式：Ctrl+C 优雅退出。
fn run_headless(cfg: config::Config) {
    let mut engine = engine::EngineHandle::new(&cfg);
    engine.start(cfg);
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");
    rt.block_on(async {
        let _ = tokio::signal::ctrl_c().await;
    });
    engine.stop();
}

/// 桌面模式：加载中文字体与主题后进入 GUI。
fn run_gui(loaded: LoadedConfig, dark: bool, minimized: bool) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([440.0, 700.0])
            .with_min_inner_size([400.0, 560.0])
            .with_icon(eframe::egui::IconData {
                width: 32,
                height: 32,
                rgba: platform::tray::app_icon_rgba(),
            }),
        ..Default::default()
    };
    eframe::run_native(
        "proxyone · 代理故障切换网关",
        options,
        Box::new(move |cc| {
            ui::install_fonts(&cc.egui_ctx);
            ui::apply_theme(&cc.egui_ctx, dark);
            let mut app = ui::App::new(loaded);
            match platform::tray::TrayHandle::new(cc.egui_ctx.clone()) {
                Ok(t) => app.set_tray(t),
                Err(e) => eprintln!("托盘初始化失败: {e:#}"),
            }
            if minimized {
                cc.egui_ctx
                    .send_viewport_cmd(eframe::egui::ViewportCommand::Visible(false));
            }
            Ok(Box::new(app))
        }),
    )
}
