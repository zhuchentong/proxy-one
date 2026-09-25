use anyhow::{Context as _, Result};
use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "failgate";
/// 项目曾用名：改名后注册表里可能残留旧值，启动时清理。
const LEGACY_VALUE_NAME: &str = "proxy-one";

fn command() -> String {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "failgate.exe".into());
    format!("\"{exe}\" --minimized")
}

fn read_value() -> Option<String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey_with_flags(RUN_KEY, KEY_QUERY_VALUE).ok()?;
    key.get_value::<String, _>(VALUE_NAME).ok()
}

pub fn is_enabled() -> bool {
    read_value().is_some()
}

/// 注册表里的命令与当前 exe 路径是否已不一致（exe 被移动/重命名后需要自愈）
pub fn stale() -> bool {
    match read_value() {
        Some(v) => v != command(),
        None => false,
    }
}

/// 删除改名前残留的旧注册表值（Run\proxy-one），避免登录时启动不存在的旧 exe。
pub fn remove_legacy() {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = hkcu.open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE) {
        let _ = key.delete_value(LEGACY_VALUE_NAME);
    }
}

pub fn set_enabled(enable: bool) -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE)
        .context("打开注册表 Run 键失败")?;
    if enable {
        key.set_value(VALUE_NAME, &command())
            .context("写入开机启动项失败")?;
    } else {
        match key.delete_value(VALUE_NAME) {
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
    fn value_name_uses_current_brand() {
        assert_eq!(VALUE_NAME, "failgate");
        assert!(command().ends_with("--minimized"));
    }
}
