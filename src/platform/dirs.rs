//! 平台目录约定：用户数据/配置目录与 XDG 基目录解析。
//!
//! 此前该逻辑在 config.rs、sysproxy_linux.rs、autostart_linux.rs 三处各写一份，
//! 现统一收敛到这里，语义不变。

use std::path::PathBuf;

/// 用户级数据目录（日志与状态文件）。
/// Windows: `%LOCALAPPDATA%\proxyone`；Linux: `$XDG_DATA_HOME/proxyone`（默认 `~/.local/share`）。
/// exe 位于 Program Files 等只读目录时，这是配置与日志的唯一可写落点。
pub(crate) fn data_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var("LOCALAPPDATA")
            .ok()
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .map(|d| d.join("proxyone"))
    }
    #[cfg(not(windows))]
    {
        xdg_base("XDG_DATA_HOME", ".local/share").map(|d| d.join("proxyone"))
    }
}

/// 用户配置目录（仅 Linux 独立存在）：`$XDG_CONFIG_HOME/proxyone`（默认 `~/.config`）。
/// Windows 的配置目录即 [`data_dir`]（`%LOCALAPPDATA%\proxyone`），调用方直接用它。
#[cfg(not(windows))]
pub(crate) fn config_dir() -> Option<PathBuf> {
    xdg_base("XDG_CONFIG_HOME", ".config").map(|d| d.join("proxyone"))
}

/// Linux XDG 配置基目录（`$XDG_CONFIG_HOME`，默认 `~/.config`），
/// 供 environment.d、autostart 等按规范挂在本目录下的子目录拼接。
#[cfg(not(windows))]
pub(crate) fn xdg_config_home() -> Option<PathBuf> {
    xdg_base("XDG_CONFIG_HOME", ".config")
}

/// XDG 基目录解析：环境变量优先，回退 `$HOME/<suffix>`；空值一律视为未设置。
#[cfg(not(windows))]
fn xdg_base(var: &str, default_suffix: &str) -> Option<PathBuf> {
    std::env::var(var)
        .ok()
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .filter(|h| !h.is_empty())
                .map(|h| PathBuf::from(h).join(default_suffix))
        })
}
