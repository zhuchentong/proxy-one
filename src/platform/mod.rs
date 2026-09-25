//! 平台抽象层：系统代理、开机启动、托盘与桌面互操作的按平台实现。
//!
//! 约定：平台差异全部收在本层，统一用「模块级 `#[cfg]` + `#[path]` 文件对」
//! 切换（该模式在 rustc 1.95 下经双平台验证，autostart/sysproxy 即此形态）；
//! 业务层只面向 `platform::` 下的同名 API，不写 `#[cfg]` 分支。
//!
//! ⚠ 例外——托盘（tray.rs）保持单文件、无 `#[cfg]`、运行时降级：rustc 1.95
//! 的 deathness 分析对「按 cfg 切换的托盘模块对」无论何种形态（自包含文件对、
//! 共享模块、re-export 链）都会 ICE（check_mod_deathness，slice index panic，
//! 多形态实测复现且与增量缓存无关）。待工具链修复后再拆分回收。

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
