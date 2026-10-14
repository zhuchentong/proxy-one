//! 系统托盘：状态图标与右键菜单。
//!
//! 右键菜单除操作项外，还以禁用项的形式展示引擎状态、当前上游与各上游的
//! 健康/延迟/流量信息；信息行由 [`TrayHandle::sync_snapshot`] 每帧刷新，
//! 上游子菜单仅在内容签名变化时重建，避免频繁增删原生菜单项。
//!
//! ℹ 本模块刻意保持单文件、无 `#[cfg]`：tray-icon 对各平台提供统一 API，
//! Windows 走原生后端，Linux 走 ksni（D-Bus SNI，后端自管工作线程、无需
//! GTK 主循环），同一份代码两端生效。menu/muda 的项对象非 `Send`，
//! [`TrayHandle::sync_snapshot`] 等菜单操作必须留在创建托盘的线程
//! （即 egui 主线程帧回调）中调用——迁移到其他线程前需重新设计。

use std::sync::mpsc::Receiver;

use anyhow::{Context as _, Result};
use eframe::egui;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::engine::{HealthStatus, Phase, Snapshot, UpstreamState};
use crate::util::fmt_bytes;

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
/// 运行中但存在故障上游（健康降级）
const ORANGE: [u8; 3] = [0xd8, 0xa0, 0x3a];

/// 托盘图标形态：绿=运行且全部健康、黄=运行但有上游故障、红=绑定失败、灰=停止。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IconKind {
    Running,
    Degraded,
    Error,
    Stopped,
}

/// 图标形态纯函数：生命周期决定基本档位，运行档再按上游健康细分降级色。
fn pick_icon_kind(phase: Phase, has_down: bool) -> IconKind {
    match phase {
        Phase::Running | Phase::Starting => {
            if has_down {
                IconKind::Degraded
            } else {
                IconKind::Running
            }
        }
        Phase::BindFailed => IconKind::Error,
        Phase::Stopping | Phase::Stopped => IconKind::Stopped,
    }
}

/// 托盘/窗口图标字形：「多路归一」——三条上游线自左汇入主干，末端实心
/// 节点表示本机统一代理入口（127.0.0.1:8888）。几何取自设计稿（视图坐标
/// 0-100），逐像素 SDF 光栅化：对每个像素取各笔画（线段/二次贝塞尔）与
/// 出口节点的最小距离换算覆盖率，得到边缘约 1px 抗锯齿的 RGBA。
/// alpha 为 straight（非预乘）：tray-icon 的 Windows 后端走 CreateIcon、
/// Linux ksni 后端仅做字节序转换，均按非预乘处理。
type Point = (f32, f32);
const STROKE_HALF_W: f32 = 5.0;
const NODE_CENTER: Point = (84.0, 50.0);
const NODE_RADIUS: f32 = 9.0;

/// 笔画线段：三条上游支线 + 汇流主干（视图坐标，距离场意义下即圆头笔帽）。
const SEGMENTS: [(Point, Point); 4] = [
    ((12.0, 28.0), (32.0, 28.0)),
    ((12.0, 50.0), (62.0, 50.0)),
    ((12.0, 72.0), (32.0, 72.0)),
    ((62.0, 50.0), (74.0, 50.0)),
];

/// 汇入主干的弯曲段（二次贝塞尔：起点、控制点、终点）。
const CURVES: [(Point, Point, Point); 2] = [
    ((32.0, 28.0), (50.0, 28.0), (62.0, 50.0)),
    ((32.0, 72.0), (50.0, 72.0), (62.0, 50.0)),
];

/// 点到线段的最短距离。
fn seg_dist(px: f32, py: f32, a: Point, b: Point) -> f32 {
    let (vx, vy) = (b.0 - a.0, b.1 - a.1);
    let len2 = vx * vx + vy * vy;
    let t = if len2 > 0.0 {
        (((px - a.0) * vx + (py - a.1) * vy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (dx, dy) = (px - (a.0 + t * vx), py - (a.1 + t * vy));
    (dx * dx + dy * dy).sqrt()
}

/// 点到二次贝塞尔曲线的最短距离：均匀采样成折线取最小值
/// （32px 渲染下 24 段的弦误差远小于半像素，足够平滑）。
fn curve_dist(px: f32, py: f32, p0: Point, p1: Point, p2: Point) -> f32 {
    const STEPS: usize = 24;
    let mut prev = p0;
    let mut best = f32::MAX;
    for i in 1..=STEPS {
        let t = i as f32 / STEPS as f32;
        let mt = 1.0 - t;
        let next = (
            mt * mt * p0.0 + 2.0 * mt * t * p1.0 + t * t * p2.0,
            mt * mt * p0.1 + 2.0 * mt * t * p1.1 + t * t * p2.1,
        );
        best = best.min(seg_dist(px, py, prev, next));
        prev = next;
    }
    best
}

/// 生成 32×32 的状态色图标 RGBA（straight alpha），托盘与窗口图标共用。
fn glyph_rgba(rgb: [u8; 3]) -> Vec<u8> {
    const S: usize = 32;
    const VIEW: f32 = 100.0;
    let scale = S as f32 / VIEW;
    let h = 0.5 / scale; // 半像素对应的视图距离：覆盖率过渡带宽约 1px
    let mut rgba = Vec::with_capacity(S * S * 4);
    for y in 0..S {
        for x in 0..S {
            let px = (x as f32 + 0.5) / scale;
            let py = (y as f32 + 0.5) / scale;
            let mut d = f32::MAX;
            for &(a, b) in &SEGMENTS {
                d = d.min(seg_dist(px, py, a, b));
            }
            for &(p0, p1, p2) in &CURVES {
                d = d.min(curve_dist(px, py, p0, p1, p2));
            }
            let d_node =
                ((px - NODE_CENTER.0).powi(2) + (py - NODE_CENTER.1).powi(2)).sqrt() - NODE_RADIUS;
            let cov_stroke = ((STROKE_HALF_W + h - d) / (2.0 * h)).clamp(0.0, 1.0);
            let cov_node = ((h - d_node) / (2.0 * h)).clamp(0.0, 1.0);
            let cov = cov_stroke.max(cov_node);
            rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], (255.0 * cov).round() as u8]);
        }
    }
    rgba
}

fn glyph_icon(rgb: [u8; 3]) -> Result<Icon> {
    let rgba = glyph_rgba(rgb);
    Icon::from_rgba(rgba, 32, 32).context("托盘图标创建失败")
}

/// 窗口图标（GUI 启动时使用，全平台可用）。
pub fn app_icon_rgba() -> Vec<u8> {
    glyph_rgba(GREEN)
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
    icon_degraded: Icon,
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
    /// 全平台统一构建：Windows 走原生后端；Linux 走 ksni（D-Bus SNI），
    /// 左键 activate → [`TrayIconEvent`]，右键菜单由托盘宿主渲染。
    pub fn new(ctx: egui::Context) -> Result<Self> {
        let icon_running = glyph_icon(GREEN)?;
        let icon_degraded = glyph_icon(ORANGE)?;
        let icon_stopped = glyph_icon(GRAY)?;
        let icon_error = glyph_icon(RED)?;

        let menu = Menu::new();
        let title_item = MenuItem::new(
            concat!("proxyone v", env!("CARGO_PKG_VERSION")),
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
            .with_tooltip("proxyone 代理故障切换网关")
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
            icon_degraded,
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
        let has_down = snap
            .upstreams
            .iter()
            .any(|u| u.status == HealthStatus::Down);
        let kind = pick_icon_kind(snap.phase, has_down);
        let icon = match kind {
            IconKind::Running => &self.icon_running,
            IconKind::Degraded => &self.icon_degraded,
            IconKind::Error => &self.icon_error,
            IconKind::Stopped => &self.icon_stopped,
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
            rate_up: 0,
            rate_down: 0,
            peak_up: 0,
            peak_down: 0,
            peak_at: None,
            checks_total: 0,
            checks_failed: 0,
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
    fn glyph_rgba_dims_and_alpha_layout() {
        let rgba = glyph_rgba(GREEN);
        assert_eq!(rgba.len(), 32 * 32 * 4);
        // 四角透明
        for (x, y) in [(0, 0), (31, 0), (0, 31), (31, 31)] {
            assert_eq!(rgba[(y * 32 + x) * 4 + 3], 0, "角像素 ({x},{y}) 应透明");
        }
        // 支线间隙透明（上游三线互不粘连）
        assert_eq!(rgba[(11 * 32 + 5) * 4 + 3], 0, "支线间隙 (5,11) 应透明");
        // 中线、主干末端与出口节点不透明
        for (x, y) in [(5, 16), (16, 16), (26, 16)] {
            assert_eq!(
                rgba[(y * 32 + x) * 4 + 3],
                255,
                "字形像素 ({x},{y}) 应不透明"
            );
        }
        // straight alpha：不透明处颜色即状态色本身
        assert_eq!(rgba[(16 * 32 + 16) * 4], GREEN[0]);
    }

    #[test]
    fn icon_kind_reflects_phase_and_upstream_health() {
        assert_eq!(pick_icon_kind(Phase::Running, false), IconKind::Running);
        assert_eq!(pick_icon_kind(Phase::Running, true), IconKind::Degraded);
        assert_eq!(pick_icon_kind(Phase::Starting, true), IconKind::Degraded);
        assert_eq!(pick_icon_kind(Phase::BindFailed, false), IconKind::Error);
        assert_eq!(pick_icon_kind(Phase::Stopped, false), IconKind::Stopped);
        assert_eq!(pick_icon_kind(Phase::Stopping, true), IconKind::Stopped);
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
