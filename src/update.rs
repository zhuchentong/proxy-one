//! 自动更新：GitHub Releases 版本检查、经网关优先的下载与原子替换。
//!
//! - 更新源 = `CARGO_PKG_REPOSITORY` 指向仓库的 latest release（不含预发布），
//!   资产为裸 `proxyone.exe` / `proxyone-linux-x64` + 对应 `.sha256`
//!   （见 .github/workflows/ci.yml）。
//! - HTTPS 走 [`crate::httpc`]（native-tls，证书校验用系统信任库）。
//! - 下载通道优先经本网关自身（享受上游故障切换），失败回退直连。
//! - 替换利用「Windows 允许改名运行中的 exe」：校验通过的字节先写成
//!   `<exe>.new`，再 当前 exe → `.old`、`.new` → 当前名、分离重启；
//!   `.old`/`.new` 残留由下次启动清理。

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::httpc::get_follow;

/// 自动检查节流：GitHub 未认证限额 60 次/时/IP，24h 一次绰绰有余
pub(crate) const CHECK_INTERVAL_SECS: u64 = 24 * 3600;
pub(crate) const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Release 资产名（见 .github/workflows/ci.yml，按平台筛选）
#[cfg(windows)]
const EXE_ASSET: &str = "proxyone.exe";
#[cfg(windows)]
const SHA_ASSET: &str = "proxyone.exe.sha256";
#[cfg(not(windows))]
const EXE_ASSET: &str = "proxyone-linux-x64";
#[cfg(not(windows))]
const SHA_ASSET: &str = "proxyone-linux-x64.sha256";

// ---------- 版本比较 ----------

/// "v0.1.0"/"0.2" → (0,1,0)/(0,2,0)；预发布后缀（如 0.3.0-rc.1）无法解析为
/// 纯数字 → None，被视为「不可比较」，因此预发布版本永远不会推给正式版用户。
fn parse_tag(tag: &str) -> Option<(u64, u64, u64)> {
    let t = tag.trim().trim_start_matches(['v', 'V']);
    if t.is_empty() {
        return None;
    }
    let mut parts = t.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Ok(0), |s| s.parse()).ok()?;
    let patch = parts.next().map_or(Ok(0), |s| s.parse()).ok()?;
    Some((major, minor, patch))
}

/// remote 是否比当前版本新
pub(crate) fn is_newer(remote: &str) -> bool {
    match (parse_tag(remote), parse_tag(CURRENT_VERSION)) {
        (Some(r), Some(c)) => r > c,
        _ => false,
    }
}

fn api_url() -> String {
    let repo = env!("CARGO_PKG_REPOSITORY").trim_end_matches('/');
    let slug = repo.rsplit_once("github.com/").map_or(repo, |(_, s)| s);
    format!("https://api.github.com/repos/{slug}/releases/latest")
}

// ---------- GitHub API ----------

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    size: u64,
    browser_download_url: String,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    assets: Vec<GhAsset>,
}

/// 可更新版本：exe 与 sha256 两个裸资产必须同时存在
#[derive(Clone)]
pub(crate) struct Release {
    pub(crate) tag: String,
    pub(crate) exe_url: String,
    pub(crate) sha_url: String,
    pub(crate) size: u64,
}

fn pick_assets(rel: &GhRelease) -> Option<Release> {
    let exe = rel.assets.iter().find(|a| a.name == EXE_ASSET)?;
    let sha = rel.assets.iter().find(|a| a.name == SHA_ASSET)?;
    Some(Release {
        tag: rel.tag_name.clone(),
        exe_url: exe.browser_download_url.clone(),
        sha_url: sha.browser_download_url.clone(),
        size: exe.size,
    })
}

/// 查询最新 release。Ok(None) = 仓库尚无发布（404）或资产不全；
/// 其余非 200 视为更新源不可用。
pub(crate) fn latest_release(gateway: Option<&str>) -> Result<Option<Release>> {
    let (status, _, body) = get_follow(
        gateway,
        &api_url(),
        "Accept: application/vnd.github+json\r\n",
        None,
    )?;
    if status == 404 {
        return Ok(None);
    }
    if status != 200 {
        bail!("GitHub API 返回 HTTP {status}");
    }
    let rel: GhRelease = serde_json::from_slice(&body).context("解析 GitHub 响应失败")?;
    Ok(pick_assets(&rel))
}

// ---------- 下载与校验 ----------

/// 下载进度：UI 直接读取，下载线程按块刷新
#[derive(Default)]
pub(crate) struct Progress {
    pub(crate) downloaded: AtomicU64,
    pub(crate) total: AtomicU64,
}

impl Progress {
    fn reset(&self) {
        self.downloaded.store(0, Ordering::Relaxed);
        self.total.store(0, Ordering::Relaxed);
    }
}

/// 后台线程 → UI 的更新事件
pub(crate) enum Msg {
    /// Ok(None) = 无新版本
    Checked(Result<Option<Release>>),
    /// 下载与校验完成，待用户确认重启应用
    Ready(Result<String>),
}

/// 下载并三重校验（长度 / sha256 / MZ 头），通过后写入 `<exe>.new`，返回版本号。
/// 不触碰当前 exe，任何失败都保持现状可回退。
pub(crate) fn download(
    gateway: Option<&str>,
    rel: &Release,
    progress: &Progress,
) -> Result<String> {
    progress.reset();
    let (_, _, sha_body) = get_follow(gateway, &rel.sha_url, "", Some(&progress.downloaded))?;
    let expect = parse_sha256(&sha_body)?;

    progress.total.store(rel.size, Ordering::Relaxed);
    progress.downloaded.store(0, Ordering::Relaxed);
    let (status, headers, exe) = get_follow(gateway, &rel.exe_url, "", Some(&progress.downloaded))?;
    if status != 200 {
        bail!("下载返回 HTTP {status}");
    }
    if let Some((_, v)) = headers.iter().find(|(k, _)| k == "content-length")
        && let Ok(cl) = v.parse::<u64>()
        && cl != exe.len() as u64
    {
        bail!("下载不完整：{}/{} 字节", exe.len(), cl);
    }
    #[cfg(windows)]
    if !exe.starts_with(b"MZ") {
        bail!("下载内容不是 Windows 可执行文件");
    }
    #[cfg(not(windows))]
    if !exe.starts_with(&[0x7f, b'E', b'L', b'F']) {
        bail!("下载内容不是 Linux 可执行文件");
    }
    let actual = format!("{:x}", Sha256::digest(&exe));
    if !actual.eq_ignore_ascii_case(&expect) {
        bail!("sha256 校验失败：预期 {expect}，实际 {actual}");
    }
    write_new(&exe)?;
    Ok(rel.tag.clone())
}

fn parse_sha256(text: &[u8]) -> Result<String> {
    let line = String::from_utf8_lossy(text);
    let tok = line
        .lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if tok.len() == 64 && tok.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(tok)
    } else {
        bail!(".sha256 内容格式异常")
    }
}

// ---------- 替换与清理 ----------

fn exe_path() -> Result<std::path::PathBuf> {
    std::env::current_exe().context("定位当前 exe 失败")
}

fn exe_sibling(suffix: &str) -> Result<PathBuf> {
    let cur = exe_path()?;
    let dir = cur.parent().ok_or_else(|| anyhow!("exe 路径异常"))?;
    let name = cur.file_name().ok_or_else(|| anyhow!("exe 路径异常"))?;
    Ok(dir.join(format!("{}{suffix}", name.to_string_lossy())))
}

fn write_new(bytes: &[u8]) -> Result<()> {
    let new_path = exe_sibling(".new")?;
    std::fs::write(&new_path, bytes).with_context(|| {
        format!(
            "写入 {} 失败（exe 目录可能只读，请手动下载更新）",
            new_path.display()
        )
    })?;
    Ok(())
}

/// 原子替换并分离启动新版本（新实例带 `--updated` 延迟启动，避开监听端口
/// 交接竞态）。成功返回后调用方应尽快退出本进程。
pub(crate) fn install() -> Result<()> {
    let cur = exe_path()?;
    let new_path = exe_sibling(".new")?;
    if !new_path.exists() {
        bail!("未找到已下载的更新文件");
    }
    #[cfg(windows)]
    {
        // Windows 锁定运行中的 exe：先改名腾出目标名，再换入新版；
        // 任一步失败都回滚改名，保住当前进程对应的可执行文件。
        let old_path = exe_sibling(".old")?;
        let _ = std::fs::remove_file(&old_path);
        std::fs::rename(&cur, &old_path).with_context(|| format!("改名 {} 失败", cur.display()))?;
        if let Err(e) = std::fs::rename(&new_path, &cur) {
            let _ = std::fs::rename(&old_path, &cur);
            return Err(anyhow!(e)).context("替换 exe 失败");
        }
    }
    #[cfg(not(windows))]
    {
        // Linux 允许对运行中的二进制做 rename 原子替换（旧 inode 由运行中的
        // 进程保活），无需改名舞步。
        std::fs::rename(&new_path, &cur).with_context(|| format!("替换 {} 失败", cur.display()))?;
    }
    std::process::Command::new(&cur)
        .arg("--updated")
        .spawn()
        .context("启动新版本失败")?;
    Ok(())
}

/// 启动时清理上一代替换残留（旧进程可能尚未退出，删除失败忽略）。
pub(crate) fn cleanup_stale() {
    let _ = std::fs::remove_file(exe_sibling(".new").unwrap_or_default());
    let _ = std::fs::remove_file(exe_sibling(".old").unwrap_or_default());
}

// ---------- 检查状态（节流与版本变化提示） ----------
// （最小 HTTPS 客户端已拆至 crate::httpc）

#[derive(Serialize, Deserialize, Default)]
struct UpdateState {
    /// 上次成功检查的 UNIX 秒
    last_check: u64,
    /// 上次启动时的版本号（变化即提示「已更新」）
    last_version: String,
}

fn state_path() -> Option<PathBuf> {
    crate::platform::dirs::data_dir().map(|d| d.join("update-state.toml"))
}

fn load_state() -> UpdateState {
    state_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_state(st: &UpdateState) {
    let Some(p) = state_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(s) = toml::to_string_pretty(st) {
        let _ = std::fs::write(p, s);
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub(crate) fn should_auto_check() -> bool {
    let st = load_state();
    st.last_check == 0 || now_unix().saturating_sub(st.last_check) >= CHECK_INTERVAL_SECS
}

/// 成功触达更新源后记录时间；失败不记录，下次启动会重试
pub(crate) fn mark_checked() {
    let mut st = load_state();
    st.last_check = now_unix();
    save_state(&st);
}

/// 写入当前版本并返回上一个版本（不同即说明发生过升级/降级）
pub(crate) fn take_version_change() -> Option<String> {
    let mut st = load_state();
    let prev = st.last_version.clone();
    st.last_version = CURRENT_VERSION.to_string();
    save_state(&st);
    (!prev.is_empty() && prev != CURRENT_VERSION).then_some(prev)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_parsing_handles_v_prefix_and_missing_parts() {
        assert_eq!(parse_tag("v0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse_tag("0.2"), Some((0, 2, 0)));
        assert_eq!(parse_tag("V1.0.0"), Some((1, 0, 0)));
        assert_eq!(parse_tag(""), None);
        // 预发布无法解析为纯数字 → 不可比较 → 不推送
        assert_eq!(parse_tag("0.3.0-rc.1"), None);
    }

    #[test]
    fn newer_only_for_strictly_greater_versions() {
        assert_eq!(CURRENT_VERSION, "0.1.0");
        assert!(is_newer("v0.1.1"));
        assert!(is_newer("v0.2"));
        assert!(is_newer("v1.0.0"));
        assert!(!is_newer("v0.1.0"));
        assert!(!is_newer("v0.0.9"));
        assert!(!is_newer("v0.1.0-beta")); // 预发布跳过
    }

    #[test]
    fn api_url_uses_repository_slug() {
        assert!(api_url().contains("/repos/zhuchentong/proxy-one/releases/latest"));
    }

    #[test]
    fn sha256_line_takes_first_hex_token() {
        let line =
            b"ABCdef0123456789ABCdef0123456789ABCdef0123456789ABCdef0123456789  proxyone.exe\n";
        assert_eq!(
            parse_sha256(line).unwrap(),
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
        );
        assert!(parse_sha256(b"short").is_err());
    }

    #[test]
    fn release_requires_both_raw_assets() {
        let mk = |name: &str| GhAsset {
            name: name.to_string(),
            size: 1,
            browser_download_url: format!("https://x/{name}"),
        };
        let full = GhRelease {
            tag_name: "v9.9.9".to_string(),
            assets: vec![mk(EXE_ASSET), mk(SHA_ASSET)],
        };
        let got = pick_assets(&full).unwrap();
        assert_eq!(got.tag, "v9.9.9");
        assert_eq!(got.size, 1);
        let missing = GhRelease {
            tag_name: "v9.9.9".to_string(),
            assets: vec![mk(EXE_ASSET)],
        };
        assert!(pick_assets(&missing).is_none());
    }
}
