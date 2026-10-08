//! Linux 系统代理：把环境变量片段写入 `~/.config/environment.d/proxyone.conf`。
//!
//! Linux 桌面没有全局"系统代理"开关——应用是否走代理取决于它启动时读到的
//! `http_proxy` 等环境变量。本实现采用 systemd 用户会话的 environment.d 机制：
//! 写入/删除片段后随即 `systemctl --user daemon-reload` 刷新用户实例环境，
//! 由 systemd 用户实例或 D-Bus 启动的应用重启即拿到新值；从终端继承 shell
//! 登录环境的应用与已运行程序不受影响（需重新登录/重启应用）。
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
    Some(
        super::dirs::xdg_config_home()?
            .join("environment.d")
            .join(FILE_NAME),
    )
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

/// 开启：写入 environment.d 片段并刷新 systemd 用户环境。
pub fn enable(listen: &str) -> Result<()> {
    let path = file_path().context("无法定位 environment.d 目录（缺少 XDG_CONFIG_HOME/HOME）")?;
    std::fs::create_dir_all(path.parent().context("路径异常")?)
        .context("创建 environment.d 目录失败")?;
    std::fs::write(&path, content(listen))
        .with_context(|| format!("写入 {} 失败", path.display()))?;
    refresh_user_manager();
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
    refresh_user_manager();
    Ok(())
}

/// 刷新 systemd 用户实例环境（`daemon-reload` 会重跑 environment-d generator）：
/// 增删片段立即反映到 manager 环境，此后由用户实例 / D-Bus activation 新启动的
/// 应用（GNOME/KDE 图标启动等多属此类）重启即拿到新值。从终端继承 shell 环境
/// 的应用不受影响（仍需重登）。失败仅告警——下次登录仍会生效。
fn refresh_user_manager() {
    match std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
    {
        Ok(s) if s.success() => {}
        Ok(s) => eprintln!("systemctl --user daemon-reload 退出码异常: {s}（下次登录仍会生效）"),
        Err(e) => eprintln!("无法执行 systemctl --user daemon-reload: {e}（下次登录仍会生效）"),
    }
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

    /// 回归：enable/disable 后 systemd 用户实例环境立即反映增删。
    /// 依赖真实 systemctl 用户实例；缺失时跳过（如无 systemd 的容器/CI）。
    /// 测试自恢复现场：无论断言成败，environment.d 状态与进入前一致。
    #[test]
    fn toggle_refreshes_systemd_user_environment() {
        let probe = std::process::Command::new("systemctl")
            .args(["--user", "is-system-running"])
            .status();
        match probe {
            Ok(s) if s.success() => {}
            _ => {
                eprintln!("跳过：无可用 systemd 用户实例");
                return;
            }
        }

        // 已有接管中的片段（如用户正在运行的实例）时不跑：测试对全局片段
        // 与 systemd 环境的独占假设不成立，活跃实例会与之竞争。
        if std::fs::read_to_string(file_path().unwrap())
            .ok()
            .is_some_and(|c| c.contains("http_proxy="))
        {
            eprintln!("跳过：检测到活跃的系统代理片段（真实实例接管中）");
            return;
        }

        let orig = std::fs::read_to_string(file_path().unwrap()).ok();
        struct Restore(Option<String>);
        impl Drop for Restore {
            fn drop(&mut self) {
                let path = file_path().unwrap();
                match &self.0 {
                    Some(c) => {
                        let _ = std::fs::write(path, c);
                    }
                    None => {
                        let _ = std::fs::remove_file(path);
                    }
                }
                let _ = std::process::Command::new("systemctl")
                    .args(["--user", "daemon-reload"])
                    .status();
            }
        }
        let _guard = Restore(orig);

        let marker = "http_proxy=http://127.0.0.1:19099";

        enable("127.0.0.1:19099").expect("enable 应成功");
        let out = std::process::Command::new("systemctl")
            .args(["--user", "show-environment"])
            .output()
            .expect("需要 systemd 用户实例");
        assert!(
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(|l| l.trim() == marker),
            "enable 后 systemd 用户环境应立即包含 {marker}"
        );

        disable("127.0.0.1:19099").expect("disable 应成功");
        let out = std::process::Command::new("systemctl")
            .args(["--user", "show-environment"])
            .output()
            .expect("需要 systemd 用户实例");
        assert!(
            !String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(|l| l.trim() == marker),
            "disable 后 systemd 用户环境应立即移除 {marker}"
        );
    }
}
