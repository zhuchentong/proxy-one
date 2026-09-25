use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpstreamKind {
    #[default]
    Auto,
    Http,
    Socks5,
}

impl UpstreamKind {
    pub fn label(self) -> &'static str {
        match self {
            UpstreamKind::Auto => "auto",
            UpstreamKind::Http => "http",
            UpstreamKind::Socks5 => "socks5",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct General {
    #[serde(default = "d_listen")]
    pub listen: String,
    #[serde(default = "d_theme")]
    pub theme: String,
    #[serde(default = "d_forward_log")]
    pub forward_log: bool,
    /// 用户意图：引擎运行时把 Windows 系统代理指向监听地址（跨重启保持）
    #[serde(default)]
    pub sysproxy: bool,
}

impl Default for General {
    fn default() -> Self {
        General {
            listen: d_listen(),
            theme: d_theme(),
            forward_log: d_forward_log(),
            sysproxy: false,
        }
    }
}

fn d_listen() -> String {
    "127.0.0.1:8888".into()
}

fn d_theme() -> String {
    "dark".into()
}

fn d_forward_log() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Health {
    #[serde(default = "d_interval")]
    pub interval_secs: u64,
    #[serde(default = "d_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "d_test_url")]
    pub test_url: String,
    #[serde(default = "d_fail")]
    pub fail_threshold: u32,
    #[serde(default = "d_success")]
    pub success_threshold: u32,
}

impl Default for Health {
    fn default() -> Self {
        Health {
            interval_secs: d_interval(),
            timeout_secs: d_timeout(),
            test_url: d_test_url(),
            fail_threshold: d_fail(),
            success_threshold: d_success(),
        }
    }
}

fn d_interval() -> u64 {
    8
}
fn d_timeout() -> u64 {
    4
}
fn d_test_url() -> String {
    "http://www.gstatic.com/generate_204".into()
}
fn d_fail() -> u32 {
    2
}
fn d_success() -> u32 {
    2
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpstreamConfig {
    pub name: String,
    pub addr: String,
    #[serde(rename = "type", default)]
    pub kind: UpstreamKind,
    pub priority: i32,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateCfg {
    /// 启动时静默检查 GitHub 新版本（每 24h 至多一次）
    #[serde(default = "d_true")]
    pub auto_check: bool,
}

impl Default for UpdateCfg {
    fn default() -> Self {
        UpdateCfg {
            auto_check: d_true(),
        }
    }
}

fn d_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: General,
    #[serde(default)]
    pub health: Health,
    #[serde(default)]
    pub update: UpdateCfg,
    #[serde(default = "default_upstreams")]
    pub upstreams: Vec<UpstreamConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            general: General::default(),
            health: Health::default(),
            update: UpdateCfg::default(),
            upstreams: default_upstreams(),
        }
    }
}

fn default_upstreams() -> Vec<UpstreamConfig> {
    vec![
        UpstreamConfig {
            name: "fmclient".into(),
            addr: "127.0.0.1:8890".into(),
            kind: UpstreamKind::Auto,
            priority: 1,
            username: None,
            password: None,
        },
        UpstreamConfig {
            name: "clash".into(),
            addr: "127.0.0.1:7890".into(),
            kind: UpstreamKind::Auto,
            priority: 2,
            username: None,
            password: None,
        },
    ]
}

pub struct LoadedConfig {
    pub config: Config,
    pub path: PathBuf,
    pub parse_error: Option<String>,
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
}

/// 用户级数据目录 `%LOCALAPPDATA%\failgate`（exe 位于 Program Files 等只读
/// 目录时，配置与日志的唯一可写落点）。
pub(crate) fn data_dir() -> Option<PathBuf> {
    std::env::var("LOCALAPPDATA")
        .ok()
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .map(|d| d.join("failgate"))
}

/// 日志目录：`%LOCALAPPDATA%\failgate\logs`。
pub(crate) fn logs_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("logs"))
}

/// 配置查找顺序：exe 同目录（便携模式）→ `%LOCALAPPDATA%\failgate` → 工作目录。
/// 都不存在时新配置写入 `%LOCALAPPDATA%`（exe 目录可能不可写）。
fn candidate_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(d) = exe_dir() {
        v.push(d.join("config.toml"));
    }
    if let Some(d) = data_dir() {
        v.push(d.join("config.toml"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        v.push(cwd.join("config.toml"));
    }
    v
}

pub fn load_or_create() -> LoadedConfig {
    let candidates = candidate_paths();
    for path in &candidates {
        if path.exists() {
            match std::fs::read_to_string(path) {
                Ok(text) => match toml::from_str::<Config>(&text) {
                    Ok(cfg) => {
                        return LoadedConfig {
                            config: cfg,
                            path: path.clone(),
                            parse_error: None,
                        };
                    }
                    Err(e) => {
                        return LoadedConfig {
                            config: Config::default(),
                            path: path.clone(),
                            parse_error: Some(e.to_string()),
                        };
                    }
                },
                Err(e) => {
                    return LoadedConfig {
                        config: Config::default(),
                        path: path.clone(),
                        parse_error: Some(format!("读取失败: {e}")),
                    };
                }
            }
        }
    }
    // 新配置默认写入 LOCALAPPDATA（兜底 exe 目录/工作目录），并预创建父目录
    let target = data_dir()
        .map(|d| d.join("config.toml"))
        .or_else(|| candidates.first().cloned())
        .unwrap_or_else(|| PathBuf::from("config.toml"));
    let cfg = Config::default();
    if let Err(e) = target.parent().map_or(Ok(()), std::fs::create_dir_all) {
        return LoadedConfig {
            config: cfg,
            path: target,
            parse_error: Some(format!("创建配置目录失败: {e}")),
        };
    }
    if let Err(e) = save(&target, &cfg) {
        return LoadedConfig {
            config: cfg,
            path: target,
            parse_error: Some(format!("默认配置写入失败: {e}")),
        };
    }
    LoadedConfig {
        config: cfg,
        path: target,
        parse_error: None,
    }
}

pub fn save(path: &Path, cfg: &Config) -> Result<()> {
    let text = toml::to_string_pretty(cfg).context("序列化配置失败")?;
    std::fs::write(path, text).with_context(|| format!("写入 {} 失败", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_roundtrips_through_toml() {
        let cfg = Config::default();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        assert_eq!(parsed, cfg);
    }

    #[test]
    fn missing_sections_fall_back_to_defaults() {
        let parsed: Config = toml::from_str("[general]\nlisten = \"127.0.0.1:9000\"\n").unwrap();
        assert_eq!(parsed.general.listen, "127.0.0.1:9000");
        assert_eq!(parsed.general.theme, "dark");
        assert_eq!(parsed.health.interval_secs, 8);
        assert_eq!(parsed.upstreams.len(), 2);
        assert_eq!(parsed.upstreams[0].name, "fmclient");
    }

    #[test]
    fn upstream_kind_parses_from_toml_strings() {
        let parsed: Config = toml::from_str(
            "[[upstreams]]\nname = \"a\"\naddr = \"127.0.0.1:1\"\ntype = \"socks5\"\npriority = 3\n",
        )
        .unwrap();
        assert_eq!(parsed.upstreams[0].kind, UpstreamKind::Socks5);
        assert_eq!(parsed.upstreams[0].priority, 3);
        assert_eq!(parsed.upstreams[0].username, None);
    }

    #[test]
    fn theme_defaults_to_dark_for_old_configs() {
        let parsed: Config = toml::from_str("[general]\nlisten = \"127.0.0.1:1\"\n").unwrap();
        assert_ne!(parsed.general.theme, "light");
    }
}
