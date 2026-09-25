//! Linux 系统代理：把环境变量片段写入 `~/.config/environment.d/proxyone.conf`。
//!
//! Linux 桌面没有全局"系统代理"开关——应用是否走代理取决于它启动时读到的
//! `http_proxy` 等环境变量。本实现采用 systemd 用户会话的 environment.d 机制：
//! 写入的变量在下次登录时注入所有用户服务与应用，关闭时删除该片段即恢复。
//! 已运行的程序不受影响（Linux 桌面的客观限制）；GNOME/KDE 的即时接管属后续阶段。
//!
//! 语义对照（与 Windows 版同名 API）：
//! - `enable`/`disable` = 写入/删除片段；
//! - `is_active` = 片段存在且指向 listen；
//! - `has_snapshot`/`restore_pending` = 无快照概念，恒为无操作
//!   （片段文件本身即是全部状态，删掉即恢复原状）。

use anyhow::{Context as _, Result};
use std::path::PathBuf;

const FILE_NAME: &str = "proxyone.conf";

fn file_path() -> Option<PathBuf> {
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
    Some(base.join("environment.d").join(FILE_NAME))
}

fn content(listen: &str) -> String {
    let listen = listen.trim();
    format!(
        "http_proxy=http://{listen}\n\
         https_proxy=http://{listen}\n\
         all_proxy=socks5://{listen}\n\
         HTTP_PROXY=http://{listen}\n\
         HTTPS_PROXY=http://{listen}\n\
         ALL_PROXY=socks5://{listen}\n\
         no_proxy=localhost,127.0.0.1,::1\n\
         NO_PROXY=localhost,127.0.0.1,::1\n"
    )
}

/// 系统代理片段当前是否由本网关写入且指向 listen。
pub fn is_active(listen: &str) -> bool {
    let marker = format!("http_proxy=http://{}", listen.trim());
    file_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|c| c.lines().any(|l| l.trim() == marker))
}

/// 开启：写入 environment.d 片段（下次登录生效）。
pub fn enable(listen: &str) -> Result<()> {
    let path = file_path().context("无法定位 environment.d 目录（缺少 XDG_CONFIG_HOME/HOME）")?;
    std::fs::create_dir_all(path.parent().context("路径异常")?)
        .context("创建 environment.d 目录失败")?;
    std::fs::write(&path, content(listen))
        .with_context(|| format!("写入 {} 失败", path.display()))?;
    Ok(())
}

/// 关闭：删除片段（不存在的忽略）。
pub fn disable(_listen: &str) -> Result<()> {
    if let Some(path) = file_path() {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context("删除 environment.d 片段失败"),
        }
    }
    Ok(())
}

/// 无快照概念：片段文件本身就是全部状态。
pub fn has_snapshot() -> bool {
    false
}

/// 无残留需要恢复。
pub fn restore_pending(_listen: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_snippet_contains_proxy_keys() {
        let c = content("127.0.0.1:8888");
        assert!(c.contains("http_proxy=http://127.0.0.1:8888"));
        assert!(c.contains("https_proxy=http://127.0.0.1:8888"));
        assert!(c.contains("all_proxy=socks5://127.0.0.1:8888"));
        assert!(c.contains("no_proxy=localhost,127.0.0.1,::1"));
    }

    #[test]
    fn file_path_ends_with_environment_d_fragment() {
        let p = file_path().expect("需要 XDG_CONFIG_HOME 或 HOME");
        assert!(p.ends_with("environment.d/proxyone.conf"));
    }
}
