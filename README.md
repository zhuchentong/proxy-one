# proxyone

[![CI](https://github.com/zhuchentong/proxy-one/actions/workflows/ci.yml/badge.svg)](https://github.com/zhuchentong/proxy-one/actions/workflows/ci.yml)

简体中文 | [English](README.en.md)

**proxyone** 是一个轻量级代理故障切换网关：本机统一代理入口 `127.0.0.1:8888`，背后按优先级聚合多个上游代理。高优先级上游失效时自动切换、恢复后自动切回——应用程序只需指向 8888，无需手动调整。

> 典型用法：本机已有多个代理客户端（如 `127.0.0.1:8890`、`127.0.0.1:7890`），proxyone 把它们聚合成一个稳定入口，谁可用用谁，应用侧只配置一次。

> Lightweight proxy failover gateway: a mixed HTTP/SOCKS5 entry that aggregates prioritized upstream proxies with health-checked automatic failover.

## 截图

<p align="center">
  <img src="images/main-dark.png" width="320" alt="主界面（深色主题）">
  &nbsp;&nbsp;&nbsp;&nbsp;
  <img src="images/settings.png" width="320" alt="独立设置页">
</p>
<p align="center"><sub>左：主界面（深色主题，运行中）；右：独立设置页</sub></p>

## 目录

- [截图](#截图)
- [功能特性](#功能特性)
- [平台支持](#平台支持)
- [快速开始](#快速开始)
- [图形界面](#图形界面)
- [系统代理](#系统代理)
- [开机启动](#开机启动)
- [自动更新](#自动更新)
- [配置文件](#配置文件)
- [工作原理](#工作原理)
- [项目结构](#项目结构)
- [构建与开发](#构建与开发)
- [已知限制](#已知限制)
- [许可证](#许可证)

## 功能特性

**核心代理**

- **混合入口**：同一端口同时支持 HTTP（CONNECT / 绝对 URI）与 SOCKS5；首字节协议嗅探，与 Clash mixed-port 同机制。
- **透明故障切换**：上游失效时，在向客户端转发任何字节之前就地切换；恢复后新连接自动切回。
- **健康检查**：周期主动探测 + 连续失败/成功滞后阈值，防止状态抖动。
- **上游认证**：SOCKS5 RFC 1929 与 HTTP Basic，按配置按需透传。

**系统集成**

- **系统代理接管**：一键将系统代理指向网关；快照恢复、崩溃自愈、冲突保护。
- **开机启动**：用户级配置，无需管理员权限；exe 移动或改名后路径自动修正。
- **自动更新**：基于 GitHub Releases；下载优先经自身网关转发（享受上游故障切换），三重完整性校验 + 原子替换。

**可观测与体验**

- **日志页**：整页日志浏览，关键字过滤 + 级别筛选（INFO / OK / WARN / ERR）。
- **统计页**：按上游汇总健康状态、累计流量、实时速率与峰值（每秒采样）。
- **单文件便携**：Rust 单二进制，egui 原生 GUI；数据面为纯 TCP 隧道盲转发。
- **深浅主题**与 `--headless` 无界面常驻模式。

## 平台支持

| 功能 | Windows | Linux |
| --- | --- | --- |
| 代理引擎（混合入口、故障切换、健康检查） | ✅ | ✅ |
| GUI | ✅ | ✅（Wayland / X11） |
| 托盘图标 | ✅ | 计划支持（当前跳过并记录日志，不影响代理） |
| 系统代理接管 | WinINet 注册表，快照/恢复/冲突保护 | 写 `~/.config/environment.d/proxyone.conf`，下次登录生效，关闭即删除恢复 |
| 开机启动 | HKCU Run 注册表 | XDG autostart `~/.config/autostart/proxyone.desktop`（sway 需会话内有 `dex -a`） |
| 自动更新 | ✅ | ✅（Linux 侧为 rename 原子替换） |
| 无界面模式 | ✅ | ✅（推荐 systemd user 服务常驻） |

## 快速开始

### 获取与运行

**方式一：下载预编译版本（推荐）**

1. 从 [Releases](https://github.com/zhuchentong/proxy-one/releases) 下载 `proxyone.exe`（Linux 下载 `proxyone-linux-x64`）；
2. 双击运行——GUI 打开，代理自动开始；首次运行会在用户配置目录生成默认配置（含两个示例上游，可按需修改）；
3. 把应用的代理设置指向 `127.0.0.1:8888`（HTTP 或 SOCKS5 均可）。

**方式二：从源码构建**

```bat
cargo build --release
```

### 命令行参数

| 参数 | 说明 |
| --- | --- |
| （无参数） | 打开 GUI，启动即自动开始代理 |
| `--headless` | 无界面常驻模式，日志打印到控制台，`Ctrl+C` 优雅退出 |
| `--minimized` | 启动后最小化到托盘（开机启动项使用） |
| `--updated` | 内部参数：自动更新替换完成后延迟启动，等待旧进程释放监听端口 |

### 验证

```bat
curl -x http://127.0.0.1:8888 https://www.google.com
curl -x http://127.0.0.1:8888 http://www.gstatic.com/generate_204
curl -x socks5h://127.0.0.1:8888 https://www.google.com
```

三条命令均应成功（仓库内 `test_curl.bat` 为以上测试的批处理版本）。

## 图形界面

### 主界面

紧凑卡片式布局，默认窗口 440×700：

- **状态卡**：运行状态、监听地址、当前上游、累计转发流量与实时速率。
- **上游卡片**：优先级徽章、名称/地址内联编辑、类型下拉（auto/http/socks5）、健康状态与延迟着色、使用中高亮；每张卡片独立统计流量与当前速率，支持上移/下移/删除。
- **测试全部**：并发测试所有上游，测试中显示旋转等待动画，完成后写结果日志并刷新延迟。
- **添加上游**：「＋ 添加」展开表单，可选择置顶优先级。
- **主题**：顶栏「☀/🌙」切换深色/浅色，立即持久化到配置。
- 所有修改点「保存并应用」后写入 `config.toml`；引擎运行中会自动以新配置重启。

### 日志与统计

- **日志面板**（主界面底部）：保留最近 200 条彩色日志，支持展开/折叠、行内选中复制、一键复制全部与清空。
- **日志页**：日志卡「↗」进入整页浏览；关键字过滤 + 级别筛选（INFO / OK / WARN / ERR 独立开关），显示命中计数，复制跟随筛选结果；「← 返回」或 `Esc` 回主界面。
- **统计页**：主界面「统计」按钮进入；按上游汇总健康状态（状态 / 延迟 / 检查次数与失败数）、累计流量与连接数、当前速率与峰值（每秒采样，峰值记录出现时刻），可一键「清零统计」。

### 托盘图标

- 三态图标：绿点 = 运行中，灰点 = 已停止，红点 = 端口绑定失败；悬停显示当前状态与监听地址。
- **左键单击**：显示 / 隐藏主窗口（隐藏后引擎继续在后台代理，不受影响）。
- **右键菜单**：显示主窗口 / 引擎运行中（勾选 = 启动，点击切换）/ 系统代理（勾选 = 接管）/ 测试全部上游 / 上游状态（各上游健康/延迟/流量）/ 打开配置文件 / 退出。
- 点击窗口 ✕ 默认隐藏到托盘；彻底退出请使用托盘菜单「退出」。

> Windows 11 下若需托盘图标常显：在任务栏「设置 > 个人设置」中开启，或将 `HKCU\Control Panel\NotifyIconSettings\<id>\IsPromoted` 设为 `1`。

### 设置页

主界面「⚙」进入独立设置页，「← 返回」或 `Esc` 退回主界面：

| 分区 | 内容 |
| --- | --- |
| 通用 | 监听地址、深色主题、转发日志、系统代理、开机启动、自动检查更新、打开日志目录、打开配置所在目录 |
| 健康检查 | 检查间隔、单次超时、连续失败/成功阈值、测试 URL |
| 关于与更新 | 当前版本、检查更新、下载进度、重启并完成更新 |

## 系统代理

设置页「通用」卡与托盘菜单均提供开关，状态跨重启保持。机制按平台不同（见「平台支持」）：Windows 即时生效；Linux 写入的 environment.d 片段在下次登录时对 systemd 用户会话生效，已运行程序不受影响。

- **开启**：WinINet 系统代理指向监听地址并广播刷新，已运行程序立即生效；开启前的原值（ProxyEnable / ProxyServer / ProxyOverride / AutoConfigURL）快照至数据目录。
- **绕过列表追加而非覆盖**：自动补充 `localhost;127.*;<local>`，用户自加条目原样保留。
- **关闭 / 退出**：恢复快照原值，而非简单置空；此前使用的其他代理（如 8890）照常还原。
- **崩溃自愈**：异常退出残留的指向本网关的系统代理，下次启动自动恢复原值；若开关原本开启，引擎启动后自动重新接管。
- **冲突保护**：仅当「已接管（快照存在）且会话中被其他程序改走」时判定为被抢占，自动放弃并关闭开关，避免与对方互相抢写。
- 引擎停止时开关自动失效；引擎恢复运行后自动重新接管。

## 开机启动

设置页「开机启动」开关，均无需管理员权限：

- **Windows**：写入当前用户注册表 Run 键（`HKCU\...\Run\proxyone`）。
- **Linux**：写入 XDG autostart 桌面项（`~/.config/autostart/proxyone.desktop`）。
- **登录行为**：自动以 `--minimized` 启动并隐藏，代理登录后直接可用。
- **路径自愈**：exe 移动位置、应用改名后，下次启动自动修正已保存路径（曾用名残留会被清理，开机启动意图自动迁移）。
- **状态以系统为准**：GUI 每次启动读取真实状态，不凭记忆显示。

> sway 注意：sway 本体不解析 XDG autostart 目录，需在 sway config 中加入 `exec dex -a`（或使用 systemd user 服务运行 `proxyone --headless`）。

## 自动更新

设置页「关于与更新」卡片可手动检查、下载与应用更新；「自动检查更新」开关（默认开启）每 24 小时在启动时静默检查一次。

- **更新源**：GitHub Releases（仓库取自 `Cargo.toml` 的 `repository` 字段），仅使用 latest release，预发布版本不会推送给用户。
- **下载通道**：引擎运行时优先经本网关自身转发下载，享受上游故障切换保护；失败自动回退直连。
- **完整性校验**：Content-Length 长度、`sha256` 校验和、PE 文件头三重校验全部通过后才落盘。
- **原子替换**：Windows 利用「允许改名运行中的 exe」完成 `当前 exe → .old`、`.new → 当前名` 的原子交换，Linux 为 rename 覆盖；随即分离启动新版本并退出，系统代理先恢复原值，新实例按持久化意图自动重新接管。
- **回滚**：`.old` 保留上一版本；exe 目录只读等异常时更新中止并给出提示，也可手动将 `.old` 改回。
- 检查 / 下载失败仅轻提示或静默处理，不影响代理主功能。

## 配置文件

### 示例

```toml
[general]
listen = "127.0.0.1:8888"

[health]
interval_secs = 8        # 检查间隔
timeout_secs = 4         # 单次探测超时
test_url = "http://www.gstatic.com/generate_204"
fail_threshold = 2       # 连续失败 N 次 → 下线
success_threshold = 2    # 连续成功 N 次 → 恢复（防抖动）

[update]
auto_check = true        # 启动时静默检查 GitHub 新版本（每 24h 至多一次）

[[upstreams]]
name = "fmclient"
addr = "127.0.0.1:8890"
type = "auto"            # auto | http | socks5
priority = 1             # 数值越小优先级越高
# username = "u"         # 可选：上游认证（SOCKS5 RFC 1929 / HTTP Basic）
# password = "p"

[[upstreams]]
name = "clash"
addr = "127.0.0.1:7890"
type = "auto"
priority = 2
```

### 加载规则

- **查找顺序**：exe 同目录（便携模式）→ 用户配置目录 → 工作目录；均不存在时自动生成于用户配置目录。
- exe 同目录存在 `config.toml` 时始终优先使用，便于便携携带；exe 位于只读目录时依然可用。
- **用户配置目录**：Windows 为 `%LOCALAPPDATA%\proxyone`，Linux 为 `$XDG_CONFIG_HOME/proxyone`（默认 `~/.config/proxyone`）。
- **数据目录**（状态文件、日志）：Windows 同配置目录，Linux 为 `$XDG_DATA_HOME/proxyone`（默认 `~/.local/share/proxyone`）。
- Windows 曾用名数据目录（`%LOCALAPPDATA%\failgate`）存在时会整体自动迁移。
- **引擎日志**落盘至 `<数据目录>/logs/proxyone.log`，超过 5MB 自动轮转为 `proxyone.log.1`；设置页提供「打开日志目录」「打开配置所在目录」快捷入口。

### 上游认证

`username` / `password` 为可选字段，留空即无认证：

- **SOCKS5 上游**：按 RFC 1929 完成用户名/密码子协商；greeting 同时提供 `0x02`（优先）与 `0x00`（兜底）两种方法，服务端任选其一均可。
- **HTTP 上游**：CONNECT 隧道与纯 HTTP 转发请求自动携带 `Proxy-Authorization: Basic`；仅经 HTTP 型上游转发时添加，不泄漏给目标站点。
- 认证失败按上游故障处理：标记 DOWN 并自动切换到下一个上游。

## 工作原理

1. **协议探测**：`type = "auto"` 时向上游发送 SOCKS5 握手包，响应 `0x05` 判为 SOCKS5，否则判为 HTTP；探测结果缓存，也可手动指定类型。
2. **主动健康检查**：每 `interval_secs` 经各上游对 `test_url` 发起 GET（期望 2xx）并记录延迟；连续失败 `fail_threshold` 次标记 DOWN，连续成功 `success_threshold` 次恢复 UP；启动时立即执行一轮。
3. **被动快速切换**：新连接拨号失败（拒连 / 握手失败）立即标记 DOWN，并在向客户端转发任何字节之前就地切换重试，同时触发一轮全量检查；经代理转发后才发现目标不可达（CONNECT 非 2xx、SOCKS5 错误码）不标记 DOWN，但同样切换。
4. **路由与切回**：每次新连接选择状态为 UP 的最高优先级上游；高优先级恢复后新连接自动切回。全部 DOWN 时仍按优先级尽力拨号。
5. **连接不迁移**：切换只影响新连接，已建立连接继续使用原上游直至结束。

## 项目结构

```text
src/
  main.rs            入口：CLI 参数（--headless / --minimized / --updated）、GUI 与 headless 启动
  config.rs          TOML 配置模型与加载/保存（带默认值回退与单元测试）
  httpc.rs           最小 HTTPS 客户端（native-tls，支持经网关 CONNECT 隧道）
  util.rs            通用小工具（字节数人性化展示等）
  update.rs          自动更新：GitHub Releases 检查、经网关优先下载、原子替换
  platform/          平台抽象层：业务层只面向统一 API，平台差异不出本层
    mod.rs           模块开关：autostart / sysproxy 按 #[cfg] + #[path] 文件对切换
    dirs.rs          用户数据/配置目录与 XDG 基目录解析
    desktop.rs       桌面互操作：文件管理器/文本编辑器打开路径
    autostart_windows.rs  开机启动（Windows）：HKCU Run 注册表读写 + 路径自愈与改名迁移
    autostart_linux.rs    开机启动（Linux）：XDG autostart 桌面项
    sysproxy_windows.rs   系统代理（Windows）：WinINet 注册表 + 快照恢复/崩溃自愈/冲突保护
    sysproxy_linux.rs     系统代理（Linux）：environment.d 片段写入
    tray.rs           托盘：三态图标/右键菜单（Windows 实装，Linux 运行时降级）
  ui/
    ui.rs            App 状态与 eframe 编排（logic 托盘/关闭拦截 + 布局）
    update_flow.rs   自动更新的 UI 编排：检查/下载后台线程与状态机
    cards.rs         主界面视图与卡片：状态卡 / 上游卡 / 添加 / 日志 / 页脚
    logs.rs          独立日志页：关键字与级别筛选、命中计数与筛选复制
    stats.rs         独立统计页：按上游汇总健康状态、流量、速率与峰值
    settings.rs      独立设置页：通用 / 健康检查 / 关于与更新
    theme.rs         明暗两套配色、Visuals 定制、中文字体加载
    widgets.rs       基础组件：卡片、胶囊徽章、拨动开关、按钮、日志着色
  engine/
    handle.rs        引擎生命周期（后台线程 + tokio runtime、启停控制）
    state.rs         共享状态：快照、200 条日志环、EngineCtx
    rates.rs         流量速率采样：每秒差分累计字节 → 当前速率与峰值
    filelog.rs       日志落盘：追加写 + 5MB 轮转
    server.rs        入站监听 + 首字节协议嗅探（0x05 SOCKS5 / HTTP / 0x04 拒绝）
    http.rs          入站 HTTP：CONNECT 隧道、绝对 URI 改写、hop-by-hop 剥离
    socks5.rs        入站 SOCKS5：无认证握手 + CONNECT
    router.rs        上游选择：优先级排序纯函数 + 连接级故障就地切换
    health.rs        健康检查循环 + 滞后阈值状态转移纯函数
    upstream.rs      上游拨号入口：协议探测/缓存 + DialError 分级
    upstream/
      dial_http.rs   HTTP CONNECT 上游拨号（含 Basic 认证）
      dial_socks5.rs SOCKS5 上游拨号（RFC 1929 用户名/密码认证）
      mock.rs        测试专用 mock 上游（仅测试构建编译）
    b64.rs           标准带填充的 Base64 编码（认证头用）
    stream.rs        隧道流工具：PrefixedStream（前缀回放）、read_head、CountingStream（流量实时记账）
    url.rs           host:port / authority / 探测 URL 解析
```

## 构建与开发

```bat
cargo build --release
```

产物为自包含单文件 `target/release/proxyone`（Windows 为 `proxyone.exe`）。单元测试：`cargo test`（覆盖配置解析、版本号比较、GitHub 资产筛选、sha256 解析、URL/authority 解析、HTTP 改写语义、路由候选排序、健康检查滞后转移、头部读取器等，另含平台专属用例）。

### Linux 构建依赖（Manjaro / Arch）

```bash
sudo pacman -S --needed rust gtk3 openssl pkgconf
cargo build --release
```

GUI 依赖 Wayland 或 X11 会话；中文字体建议安装 `noto-fonts-cjk`。Debian/Ubuntu 系对应包为 `libgtk-3-dev pkg-config libssl-dev`。

### 开发环境说明

- GUI 渲染使用 **wgpu**（DX12/Vulkan，兼容远程桌面）；glow（OpenGL）实测存在窗口白屏问题——不仅限远程会话，本地 NVIDIA 驱动同样复现（2026-09 二次验证后回退），勿再切回。
- 本仓库使用 GNU 工具链构建（目录级 `rustup override`）；如需切回 MSVC，请在 VS Installer 中为 Visual Studio 安装「使用 C++ 的桌面开发」工作负载，然后执行 `rustup override unset`。
- crates 镜像：TUN 虚拟网卡环境下 cargo 到部分镜像源的 TLS 握手会被拦截，项目级 `.cargo/config.toml` 已改用阿里云镜像（仅影响本仓库）。

## 已知限制

**代理语义**

- 不代理 UDP（SOCKS5 仅支持 CONNECT，BIND / UDP ASSOCIATE 明确拒绝）
- 入站 SOCKS5 仅支持无认证（仅监听回环地址，本地使用无需认证）；SOCKS4 请求被拒绝
- 纯 HTTP 转发强制 `Connection: close`（客户端每请求一条连接；浏览器与 WebSocket 不受影响，ws 走 CONNECT）
- 健康检查 `test_url` 仅支持 `http://`（默认 generate_204 即为 http）

**平台与部署**

- Linux 托盘图标暂未支持（阶段 3 计划，倾向 KSNI 路线；当前 GUI 正常运行，仅无托盘）
- Linux 系统代理为 environment.d 片段（下次登录生效），不支持已运行程序的即时接管
- 默认 8s 间隔的健康检查对每个上游每天约 1 万次 204 探测，量级无害；如需调整可在 GUI 修改检查间隔

## 许可证

本项目采用 [MIT](LICENSE-MIT) 与 [Apache-2.0](LICENSE-APACHE) 双许可，任选其一遵循即可。
