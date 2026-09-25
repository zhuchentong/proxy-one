//! Linux 开机启动：写 `~/.config/autostart/proxyone.desktop`（XDG autostart 规范）。
//!
//! GNOME/KDE 等桌面原生解析该目录；sway 本体不解析，需会话中存在
//! `dex -a` 或等效机制（如 sway config 里 `exec dex -a`）。
//! exe 移动/重命名后内容与当前路径不一致时由启动流程按 `stale()` 自愈重写。

use anyhow::{Context as _, Result};
use std::path::PathBuf;

const FILE_NAME: &str = "proxyone.desktop";
/// 项目曾用名的桌面文件，启动时清理。
const LEGACY_FILE_NAME: &str = "failgate.desktop";

fn autostart_dir() -> Option<PathBuf> {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .filter(|h| !h.is_empty())
                .map(|h| PathBuf::from(h).join(".config"))
        })?;
    Some(base.join("autostart"))
}

fn file_path() -> Option<PathBuf> {
    autostart_dir().map(|d| d.join(FILE_NAME))
}

fn command() -> String {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "proxyone".into());
    format!("{exe} --minimized")
}

fn desktop_entry() -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=proxyone\nExec={}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        command()
    )
}

fn read_entry() -> Option<String> {
    let path = file_path()?;
    std::fs::read_to_string(path).ok()
}

pub fn is_enabled() -> bool {
    read_entry().is_some()
}

/// .desktop 内容与当前 exe 路径是否已不一致（exe 被移动/重命名后需要自愈）
pub fn stale() -> bool {
    match read_entry() {
        Some(existing) => existing != desktop_entry(),
        None => false,
    }
}

/// 清理曾用名残留的 .desktop 文件。
pub fn remove_legacy() {
    if let Some(path) = autostart_dir().map(|d| d.join(LEGACY_FILE_NAME)) {
        let _ = std::fs::remove_file(path);
    }
}

pub fn set_enabled(enable: bool) -> Result<()> {
    let path = file_path().context("无法定位 autostart 目录（缺少 XDG_CONFIG_HOME/HOME）")?;
    if enable {
        std::fs::create_dir_all(path.parent().context("路径异常")?)
            .context("创建 autostart 目录失败")?;
        std::fs::write(&path, desktop_entry())
            .with_context(|| format!("写入 {} 失败", path.display()))?;
    } else {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context("删除开机启动项失败"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_uses_current_brand_and_minimized() {
        let entry = desktop_entry();
        assert!(entry.starts_with("[Desktop Entry]"));
        assert!(entry.contains("Name=proxyone"));
        assert!(entry.trim_end().ends_with("X-GNOME-Autostart-enabled=true"));
        assert!(command().ends_with("--minimized"));
    }
}
