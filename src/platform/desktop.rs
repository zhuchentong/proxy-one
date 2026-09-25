//! 桌面互操作：用系统文件管理器 / 文本编辑器打开路径。
//!
//! 自 ui.rs 收编——这是平台能力而非 UI 编排逻辑；后续若接 wslpath 或
//! GNOME/KDE 专用打开方式，落点也在本文件。

use std::path::Path;

/// 用文件管理器打开目录（Windows: explorer；Linux: xdg-open）。
pub fn open_path(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    return std::process::Command::new("explorer.exe")
        .arg(path)
        .spawn()
        .map(|_| ());
    #[cfg(not(windows))]
    return std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map(|_| ());
}

/// 用系统文本编辑器打开配置文件。
pub fn edit_text_file(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    return std::process::Command::new("notepad.exe")
        .arg(path)
        .spawn()
        .map(|_| ());
    #[cfg(not(windows))]
    return std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map(|_| ());
}

/// 在文件管理器中定位文件（Linux 退化为打开所在目录）。
pub fn reveal_path(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    return std::process::Command::new("explorer.exe")
        .arg(format!("/select,\"{}\"", path.display()))
        .spawn()
        .map(|_| ());
    #[cfg(not(windows))]
    return std::process::Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .spawn()
        .map(|_| ());
}
