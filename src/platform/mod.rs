//! 平台抽象层：系统代理、开机启动、托盘与桌面互操作的按平台实现。
//!
//! 约定：平台差异全部收在本层，统一用「模块级 `#[cfg]` + `#[path]` 文件对」
//! 切换（该模式在 rustc 1.95 下经双平台验证，autostart/sysproxy 即此形态）；
//! 业务层只面向 `platform::` 下的同名 API，不写 `#[cfg]` 分支。
//!
//! 托盘（tray.rs）是唯一单文件形态：tray-icon 各平台 API 统一，无需按
//! 平台拆分（详见该文件头说明）。

#[cfg(windows)]
#[path = "autostart_windows.rs"]
pub mod autostart;
#[cfg(not(windows))]
#[path = "autostart_linux.rs"]
pub mod autostart;

#[cfg(windows)]
#[path = "sysproxy_windows.rs"]
pub mod sysproxy;
#[cfg(not(windows))]
#[path = "sysproxy_linux.rs"]
pub mod sysproxy;

pub mod desktop;
pub mod dirs;
pub mod tray;
