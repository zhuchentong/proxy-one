//! 异步代理引擎。
//!
//! 数据通路：[`server`] 接收入站连接并按首字节嗅探协议（0x05 → SOCKS5，
//! 其余 → HTTP），[`http`]/[`socks5`] 解析出目标地址后交给 [`router`]，
//! 由 [`upstream`] 按配置拨号建立隧道；[`health`] 周期性经每个上游探测，
//! 驱动 UP/DOWN 状态的滞后切换。
//!
//! 与 GUI 的交互：[`state::StateStore`] 持有全部可观察状态（快照 + 日志），
//! [`handle::EngineHandle`] 负责后台线程与 runtime 的启停控制。

pub mod b64;
pub mod filelog;
pub mod handle;
pub mod health;
pub mod http;
pub mod router;
pub mod server;
pub mod socks5;
pub mod state;
pub mod stream;
pub mod upstream;
pub mod url;

pub use handle::EngineHandle;
pub use state::{EngineCtx, HealthStatus, LogLevel, Phase, Snapshot, UpstreamState};
