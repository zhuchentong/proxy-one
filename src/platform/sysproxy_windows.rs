//! Windows 系统代理开关：写 WinINet 注册表键并广播刷新。
//!
//! 语义与健壮性（经交叉验证确定）：
//! - 开启前把 `(ProxyEnable, ProxyServer, ProxyOverride, AutoConfigURL)` 四元组
//!   快照到数据目录，关闭时**恢复快照**而非简单置 0（用户可能原本开着别的代理）；
//! - 已有快照时不覆盖（避免把"我们自己的代理"当成原始值）；
//! - 关闭前校验归属：仅当 `ProxyServer` 仍指向自己时才恢复；
//! - 异常退出残留：启动时若存在快照且系统代理未被接管，自动恢复；
//! - 每次注册表变更后调用 `InternetSetOptionW` 广播，已运行的 WinINet 程序立即生效。

use anyhow::{Context as _, Result};
use std::path::PathBuf;
use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE};

use windows_sys::Win32::Networking::WinInet::{
    INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED, InternetSetOptionW,
};

const INET_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
const V_PROXY_ENABLE: &str = "ProxyEnable";
const V_PROXY_SERVER: &str = "ProxyServer";
const V_PROXY_OVERRIDE: &str = "ProxyOverride";
const V_PAC: &str = "AutoConfigURL";
/// 绕过列表中必须包含的条目（指向本网关时回环流量不能进代理）
const REQUIRED_BYPASS: [&str; 3] = ["localhost", "127.*", "<local>"];

/// 开启前的系统代理快照，落盘于数据目录 `sysproxy-state.toml`。
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Snapshot {
    enable: bool,
    server: String,
    #[serde(default)]
    bypass: String,
    #[serde(default)]
    pac: String,
}

fn inet_key(writable: bool) -> Result<RegKey> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let flags = if writable {
        KEY_SET_VALUE | KEY_QUERY_VALUE
    } else {
        KEY_QUERY_VALUE
    };
    hkcu.open_subkey_with_flags(INET_KEY, flags)
        .context("打开 Internet Settings 注册表键失败")
}

fn snapshot_path() -> Option<PathBuf> {
    crate::platform::dirs::data_dir().map(|d| d.join("sysproxy-state.toml"))
}

fn load_snapshot() -> Option<Snapshot> {
    let path = snapshot_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    toml::from_str(&text).ok()
}

fn save_snapshot(snap: &Snapshot) -> Result<()> {
    let path = snapshot_path().context("数据目录不可用")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("创建数据目录失败")?;
    }
    std::fs::write(&path, toml::to_string(snap)?).context("写入代理快照失败")?;
    Ok(())
}

fn delete_snapshot() {
    if let Some(path) = snapshot_path() {
        let _ = std::fs::remove_file(path);
    }
}

/// 快照是否已存在（开启过且未恢复）。
pub fn has_snapshot() -> bool {
    snapshot_path().map(|p| p.exists()).unwrap_or(false)
}

fn get_string(key: &RegKey, name: &str) -> Option<String> {
    key.get_value::<String, _>(name).ok()
}

fn set_or_delete_string(key: &RegKey, name: &str, value: Option<&str>) -> Result<()> {
    match value {
        Some(v) => key
            .set_value(name, &v)
            .with_context(|| format!("写入 {name} 失败"))?,
        None => match key.delete_value(name) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("删除 {name} 失败")),
        },
    }
    Ok(())
}

/// 系统代理当前是否由本网关接管（ProxyEnable=1 且 ProxyServer 指向 listen）。
pub fn is_active(listen: &str) -> bool {
    inet_key(false).ok().is_some_and(|key| {
        key.get_value::<u32, _>(V_PROXY_ENABLE).ok() == Some(1)
            && get_string(&key, V_PROXY_SERVER).as_deref() == Some(listen)
    })
}

/// 开启：快照当前值（已有快照时不覆盖）→ 指向 listen → 广播。
pub fn enable(listen: &str) -> Result<()> {
    let key = inet_key(true)?;
    if !has_snapshot() {
        save_snapshot(&Snapshot {
            enable: key.get_value::<u32, _>(V_PROXY_ENABLE).ok() == Some(1),
            server: get_string(&key, V_PROXY_SERVER).unwrap_or_default(),
            bypass: get_string(&key, V_PROXY_OVERRIDE).unwrap_or_default(),
            pac: get_string(&key, V_PAC).unwrap_or_default(),
        })?;
    }
    // 追加（而非覆盖）绕过列表，保留用户自加的条目
    let mut entries: Vec<String> = get_string(&key, V_PROXY_OVERRIDE)
        .unwrap_or_default()
        .split(';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    for want in REQUIRED_BYPASS {
        if !entries.iter().any(|e| e.eq_ignore_ascii_case(want)) {
            entries.push(want.to_string());
        }
    }
    key.set_value(V_PROXY_OVERRIDE, &entries.join(";"))
        .context("写入 ProxyOverride 失败")?;
    // PAC 优先于 ProxyEnable，接管期间必须摘除
    set_or_delete_string(&key, V_PAC, None)?;
    key.set_value(V_PROXY_SERVER, &listen)
        .context("写入 ProxyServer 失败")?;
    key.set_value(V_PROXY_ENABLE, &1u32)
        .context("写入 ProxyEnable 失败")?;
    broadcast();
    Ok(())
}

/// 关闭：仍由本网关接管时恢复快照；随后删除快照并广播。
pub fn disable(listen: &str) -> Result<()> {
    let key = inet_key(true)?;
    if is_active(listen) {
        if let Some(snap) = load_snapshot() {
            key.set_value(V_PROXY_ENABLE, &(if snap.enable { 1u32 } else { 0u32 }))
                .context("恢复 ProxyEnable 失败")?;
            let server = (!snap.server.is_empty()).then_some(snap.server.as_str());
            set_or_delete_string(&key, V_PROXY_SERVER, server)?;
            let bypass = (!snap.bypass.is_empty()).then_some(snap.bypass.as_str());
            set_or_delete_string(&key, V_PROXY_OVERRIDE, bypass)?;
            let pac = (!snap.pac.is_empty()).then_some(snap.pac.as_str());
            set_or_delete_string(&key, V_PAC, pac)?;
        } else {
            // 无快照兜底：至少摘掉指向自己的代理
            key.set_value(V_PROXY_ENABLE, &0u32)
                .context("写入 ProxyEnable 失败")?;
        }
    }
    delete_snapshot();
    broadcast();
    Ok(())
}

/// 崩溃/异常退出后的启动自愈：存在快照且系统代理仍指向自己 → 恢复原值。
/// 若用户事后手动改成了别的代理，则仅清理过期的快照文件。
pub fn restore_pending(listen: &str) {
    if !has_snapshot() {
        return;
    }
    if let Err(e) = disable(listen) {
        eprintln!("恢复系统代理快照失败: {e:#}");
    }
}

fn broadcast() {
    unsafe {
        InternetSetOptionW(
            std::ptr::null(),
            INTERNET_OPTION_SETTINGS_CHANGED,
            std::ptr::null(),
            0,
        );
        InternetSetOptionW(
            std::ptr::null(),
            INTERNET_OPTION_REFRESH,
            std::ptr::null(),
            0,
        );
    }
}
