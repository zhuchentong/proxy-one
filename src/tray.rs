//! 系统托盘：状态图标与右键菜单。
//!
//! 右键菜单除操作项外，还以禁用项的形式展示引擎状态、当前上游与各上游的
//! 健康/延迟/流量信息；信息行由 [`TrayHandle::sync_snapshot`] 每帧刷新，
//! 上游子菜单仅在内容签名变化时重建，避免频繁增删原生菜单项。

use std::sync::mpsc::Receiver;

use anyhow::{Context as _, Result};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::engine::{HealthStatus, Phase, Snapshot, UpstreamState};
use crate::ui::widgets::fmt_bytes;

pub enum TrayMsg {
    ShowWindow,
    ToggleEngine,
    ToggleSysProxy,
    TestAll,
    OpenConfig,
    Quit,
}

const GREEN: [u8; 3] = [0x3f, 0xc1, 0x7a];
const GRAY: [u8; 3] = [0x8a, 0x8a, 0x8a];
const RED: [u8; 3] = [0xe0, 0x60, 0x5d];

fn circle_rgba(rgb: [u8; 3]) -> Vec<u8> {
    let s = 32usize;
    let c = (s as f32 - 1.0) / 2.0;
    let mut rgba = Vec::with_capacity(s * s * 4);
    for y in 0..s {
        for x in 0..s {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            let px = if d <= 11.0 {
                [rgb[0], rgb[1], rgb[2], 255]
            } else if d <= 13.5 {
                [rgb[0] / 2, rgb[1] / 2, rgb[2] / 2, 255]
            } else {
                [0, 0, 0, 0]
            };
            rgba.extend_from_slice(&px);
        }
    }
    rgba
}

fn circle_icon(rgb: [u8; 3]) -> Result<Icon> {
    let rgba = circle_rgba(rgb);
    Icon::from_rgba(rgba, 32, 32).context("托盘图标创建失败")
}

pub fn app_icon_rgba() -> Vec<u8> {
    circle_rgba(GREEN)
}

/// 引擎状态行：● 运行中 · 监听地址（有故障上游时附计数）。
fn status_line(snap: &Snapshot) -> String {
    match snap.phase {
        Phase::Running | Phase::Starting => {
            let downs = snap
                .upstreams
                .iter()
                .filter(|u| u.status == HealthStatus::Down)
                .count();
            let extra = if downs > 0 {
                format!(" · {downs} 个上游故障")
            } else {
                String::new()
            };
            format!("● 运行中 · {}{extra}", snap.listen)
        }
        Phase::BindFailed => format!("× 端口绑定失败 · {}", snap.listen),
        Phase::Stopping => "○ 正在停止…".to_string(),
        Phase::Stopped => "○ 已停止".to_string(),
    }
}

/// 当前上游行（仅运行中才有意义）。
fn active_line(snap: &Snapshot) -> String {
    match (&snap.phase, &snap.active) {
        (Phase::Running | Phase::Starting, Some(n)) => format!("当前上游: {n}"),
        (Phase::Running | Phase::Starting, None) => "当前上游: 尚无转发".to_string(),
        _ => "当前上游: —".to_string(),
    }
}

/// 上游子菜单中单条上游的信息行。
fn upstream_menu_text(u: &UpstreamState) -> String {
    let (dot, state) = match u.status {
        HealthStatus::Up => (
            "●",
            match u.latency_ms {
                Some(ms) => format!("健康 {ms}ms"),
                None => "健康".to_string(),
            },
        ),
        HealthStatus::Down => ("×", "故障".to_string()),
        HealthStatus::Unknown => ("○", "未知".to_string()),
    };
    let mut s = format!("{dot} {} · {}", u.name, state);
    if u.conns > 0 {
        s.push_str(&format!(
            " · ↑{} ↓{}",
            fmt_bytes(u.bytes_up),
            fmt_bytes(u.bytes_down)
        ));
    }
    s
}

/// 上游子菜单内容的签名：任一上游状态/延迟/流量变化时才重建子菜单。
fn menu_signature(snap: &Snapshot) -> String {
    let mut key = format!("{:?}|{}|{:?}", snap.phase, snap.listen, snap.active);
    for u in &snap.upstreams {
        key.push_str(&format!(
            "|{}|{:?}|{:?}|{}|{}|{}",
            u.name, u.status, u.latency_ms, u.conns, u.bytes_up, u.bytes_down
        ));
    }
    key
}

pub struct TrayHandle {
    msg_rx: Receiver<TrayMsg>,
    tray: TrayIcon,
    icon_running: Icon,
    icon_stopped: Icon,
    icon_error: Icon,
    engine_item: CheckMenuItem,
    sysproxy_item: CheckMenuItem,
    status_item: MenuItem,
    active_item: MenuItem,
    upstream_menu: Submenu,
    last_tip: Option<String>,
    last_menu_key: Option<String>,
    last_sysproxy_checked: bool,
}

impl TrayHandle {
    pub fn new(ctx: egui::Context) -> Result<Self> {
        let icon_running = circle_icon(GREEN)?;
        let icon_stopped = circle_icon(GRAY)?;
        let icon_error = circle_icon(RED)?;

        let menu = Menu::new();
        let title_item = MenuItem::new(
            concat!("failgate v", env!("CARGO_PKG_VERSION")),
            false,
            None,
        );
        let status_item = MenuItem::new("○ 已停止", false, None);
        let active_item = MenuItem::new("当前上游: —", false, None);
        let show_item = MenuItem::new("显示主窗口", true, None);
        let engine_item = CheckMenuItem::new("引擎运行中", true, false, None);
        let sysproxy_item = CheckMenuItem::new("系统代理", true, false, None);
        let test_item = MenuItem::new("测试全部上游", true, None);
        let open_cfg_item = MenuItem::new("打开配置文件", true, None);
        let quit_item = MenuItem::new("退出", true, None);
        let upstream_menu = Submenu::new("上游状态", true);
        menu.append(&title_item)?;
        menu.append(&status_item)?;
        menu.append(&active_item)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&show_item)?;
        menu.append(&engine_item)?;
        menu.append(&sysproxy_item)?;
        menu.append(&test_item)?;
        menu.append(&open_cfg_item)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&upstream_menu)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&quit_item)?;

        let tray = TrayIconBuilder::new()
            .with_tooltip("failgate 代理故障切换网关")
            .with_icon(icon_stopped.clone())
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .context("托盘图标创建失败")?;

        let (tx, rx) = std::sync::mpsc::channel::<TrayMsg>();
        let show_id = show_item.id().clone();
        let engine_id = engine_item.id().clone();
        let sysproxy_id = sysproxy_item.id().clone();
        let test_id = test_item.id().clone();
        let open_cfg_id = open_cfg_item.id().clone();
        let quit_id = quit_item.id().clone();

        let ctx_click = ctx.clone();
        let tx_click = tx.clone();
        TrayIconEvent::set_event_handler(Some(move |ev: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                ..
            } = ev
            {
                let _ = tx_click.send(TrayMsg::ShowWindow);
                ctx_click.request_repaint();
            }
        }));

        let ctx_menu = ctx.clone();
        MenuEvent::set_event_handler(Some(move |ev: MenuEvent| {
            let msg = if ev.id == show_id {
                Some(TrayMsg::ShowWindow)
            } else if ev.id == engine_id {
                Some(TrayMsg::ToggleEngine)
            } else if ev.id == sysproxy_id {
                Some(TrayMsg::ToggleSysProxy)
            } else if ev.id == test_id {
                Some(TrayMsg::TestAll)
            } else if ev.id == open_cfg_id {
                Some(TrayMsg::OpenConfig)
            } else if ev.id == quit_id {
                Some(TrayMsg::Quit)
            } else {
                None
            };
            if let Some(m) = msg {
                let _ = tx.send(m);
                ctx_menu.request_repaint();
            }
        }));

        Ok(Self {
            msg_rx: rx,
            tray,
            icon_running,
            icon_stopped,
            icon_error,
            engine_item,
            sysproxy_item,
            status_item,
            active_item,
            upstream_menu,
            last_tip: None,
            last_menu_key: None,
            last_sysproxy_checked: false,
        })
    }

    pub fn try_recv(&self) -> Option<TrayMsg> {
        self.msg_rx.try_recv().ok()
    }

    /// 同步「系统代理」勾选状态（有变化才写原生菜单）。
    pub fn set_sysproxy_checked(&mut self, on: bool) {
        if self.last_sysproxy_checked != on {
            self.sysproxy_item.set_checked(on);
            self.last_sysproxy_checked = on;
        }
    }

    /// 按引擎快照刷新图标、tooltip 与菜单信息；在 GUI 帧回调中调用
    /// （窗口隐藏时依然执行，保证托盘信息不滞后）。
    pub fn sync_snapshot(&mut self, snap: &Snapshot) {
        let running = matches!(snap.phase, Phase::Running | Phase::Starting);
        let icon = match snap.phase {
            Phase::Running | Phase::Starting => &self.icon_running,
            Phase::BindFailed => &self.icon_error,
            Phase::Stopping | Phase::Stopped => &self.icon_stopped,
        };
        let tip = status_line(snap);
        if self.last_tip.as_deref() != Some(&tip) {
            let _ = self.tray.set_icon(Some(icon.clone()));
            let _ = self.tray.set_tooltip(Some(tip.clone()));
            self.last_tip = Some(tip);
        }
        self.engine_item.set_checked(running);

        self.status_item.set_text(status_line(snap));
        self.active_item.set_text(active_line(snap));

        let key = menu_signature(snap);
        if self.last_menu_key.as_deref() != Some(&key) {
            self.last_menu_key = Some(key);
            self.rebuild_upstream_menu(snap);
        }
    }

    fn rebuild_upstream_menu(&mut self, snap: &Snapshot) {
        while self.upstream_menu.remove_at(0).is_some() {}
        if snap.upstreams.is_empty() {
            let item = MenuItem::new("（无上游）", false, None);
            let _ = self.upstream_menu.append(&item);
            return;
        }
        for u in &snap.upstreams {
            let item = MenuItem::new(upstream_menu_text(u), false, None);
            let _ = self.upstream_menu.append(&item);
        }
        let tot_conns: u64 = snap.upstreams.iter().map(|u| u.conns).sum();
        if tot_conns > 0 {
            let tot_up: u64 = snap.upstreams.iter().map(|u| u.bytes_up).sum();
            let tot_down: u64 = snap.upstreams.iter().map(|u| u.bytes_down).sum();
            let _ = self.upstream_menu.append(&PredefinedMenuItem::separator());
            let item = MenuItem::new(
                format!(
                    "合计 ↑{} ↓{} · {} 次连接",
                    fmt_bytes(tot_up),
                    fmt_bytes(tot_down),
                    tot_conns
                ),
                false,
                None,
            );
            let _ = self.upstream_menu.append(&item);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upstream(name: &str, status: HealthStatus, latency: Option<u64>) -> UpstreamState {
        UpstreamState {
            name: name.to_string(),
            status,
            detected: None,
            latency_ms: latency,
            last_check: None,
            fail_streak: 0,
            ok_streak: 0,
            conns: 0,
            bytes_up: 0,
            bytes_down: 0,
            testing: false,
        }
    }

    fn snapshot(phase: Phase, upstreams: Vec<UpstreamState>, active: Option<&str>) -> Snapshot {
        Snapshot {
            phase,
            listen: "127.0.0.1:8888".into(),
            error: None,
            upstreams,
            active: active.map(|s| s.to_string()),
            logs: Vec::new(),
        }
    }

    #[test]
    fn status_line_shows_running_and_failures() {
        let mut snap = snapshot(
            Phase::Running,
            vec![
                upstream("a", HealthStatus::Up, Some(50)),
                upstream("b", HealthStatus::Down, None),
            ],
            Some("a"),
        );
        assert_eq!(
            status_line(&snap),
            "● 运行中 · 127.0.0.1:8888 · 1 个上游故障"
        );
        snap.upstreams[1].status = HealthStatus::Up;
        assert_eq!(status_line(&snap), "● 运行中 · 127.0.0.1:8888");
        snap.phase = Phase::Stopped;
        assert_eq!(status_line(&snap), "○ 已停止");
        snap.phase = Phase::BindFailed;
        assert_eq!(status_line(&snap), "× 端口绑定失败 · 127.0.0.1:8888");
    }

    #[test]
    fn active_line_follows_phase() {
        let snap = snapshot(Phase::Running, vec![], Some("fmclient"));
        assert_eq!(active_line(&snap), "当前上游: fmclient");
        let snap = snapshot(Phase::Running, vec![], None);
        assert_eq!(active_line(&snap), "当前上游: 尚无转发");
        let snap = snapshot(Phase::Stopped, vec![], Some("fmclient"));
        assert_eq!(active_line(&snap), "当前上游: —");
    }

    #[test]
    fn upstream_text_shows_status_latency_traffic() {
        let mut u = upstream("fmclient", HealthStatus::Up, Some(53));
        assert_eq!(upstream_menu_text(&u), "● fmclient · 健康 53ms");
        u.conns = 6;
        u.bytes_up = 3891;
        u.bytes_down = 95_600;
        assert_eq!(
            upstream_menu_text(&u),
            "● fmclient · 健康 53ms · ↑3.8 KB ↓93.4 KB"
        );
        assert_eq!(
            upstream_menu_text(&upstream("clash", HealthStatus::Down, None)),
            "× clash · 故障"
        );
        assert_eq!(
            upstream_menu_text(&upstream("new", HealthStatus::Unknown, None)),
            "○ new · 未知"
        );
    }

    #[test]
    fn menu_signature_tracks_state_changes() {
        let snap = snapshot(
            Phase::Running,
            vec![upstream("a", HealthStatus::Up, Some(50))],
            Some("a"),
        );
        let base = menu_signature(&snap);

        let mut changed = snap.clone();
        changed.upstreams[0].latency_ms = Some(60);
        assert_ne!(base, menu_signature(&changed));

        let mut changed = snap.clone();
        changed.upstreams[0].bytes_down = 128;
        assert_ne!(base, menu_signature(&changed));

        let mut changed = snap.clone();
        changed.upstreams[0].last_check = Some("12:00:00".into());
        assert_eq!(base, menu_signature(&changed));
    }
}
