# failgate — 代理故障切换网关（Rust + GUI）

本机统一代理入口 **127.0.0.1:8888**，背后按优先级聚合多个上游代理。高优先级上游失效时自动切换、恢复后自动切回，应用只需指向 8888，永远不用手动改。

- 入口为 **混合端口**：同一端口同时支持 HTTP 代理（CONNECT / 绝对 URI）与 SOCKS5（读首字节嗅探，与 clash mixed-port 同机制）
- 纯 TCP 代理（覆盖浏览器与绝大多数软件的用法），数据面为隧道盲转发
- 单文件 exe（egui/eframe GUI），配置为同目录 `config.toml`

## 快速开始

```text
failgate.exe            # 打开 GUI（启动即自动开始代理）
failgate.exe --headless # 无界面常驻（日志打印到控制台，Ctrl+C 优雅退出）
```

GUI 操作：紧凑卡片式布局（默认窗口 440×700），顶栏「☀/🌙」一键切换**深色/浅色主题**（立即持久化到 config.toml 的 `general.theme`，重启保持）；状态卡展示运行状态、监听地址、当前上游与累计转发流量（上行/下行/连接数）；上游以卡片呈现（含各自累计流量统计；优先级徽章、内联编辑名称/地址、类型下拉、健康状态与延迟着色、使用中卡片高亮，支持上移/下移/删除）；上游区右上角**「测试全部」并发测试所有上游**（每个上游各自显示旋转等待动画与「测试中」，完成后写结果日志并刷新延迟）；「＋ 添加」展开添加卡片；「⚙」打开**独立设置页**（返回按钮 / Esc 退回主界面；「通用」：监听地址、深色主题、转发日志、系统代理、开机启动、自动检查更新、打开日志目录/配置所在目录；「健康检查」：检查间隔、超时、连续失败/成功阈值、测试 URL；「关于与更新」：当前版本、检查更新、下载进度与重启应用）；底部日志卡片保留最近 200 条彩色日志，支持展开/折叠、行内选中复制、一键「复制全部」与「清空」。所有修改点「保存并应用」即写入 config.toml，若引擎在运行会自动以新配置重启。

## 托盘图标

- 程序启动后在系统托盘常驻：**绿点 = 运行中，灰点 = 已停止，红点 = 端口绑定失败**；悬停提示当前状态与监听地址。
- **左键单击**托盘图标：显示 / 隐藏主窗口（隐藏后引擎继续在后台代理，不受影响）。
- **右键**托盘图标：菜单「显示主窗口 / 引擎运行中（勾选 = 启动，点击切换启停）/ **系统代理**（勾选 = 把 Windows 系统代理指向本网关）/ 测试全部上游 / 上游状态（各上游健康/延迟/流量）/ 打开配置文件 / 退出」。
- **点击窗口 ✕ 默认隐藏到托盘**而非退出；彻底退出请用托盘菜单「退出」。
- 图标默认收在任务栏溢出区（^ 内）；如需常显：任务栏 设置 > 个人设置，或将 `HKCU\Control Panel\NotifyIconSettings\<id>\IsPromoted` 设为 1（本项目已按此设置）。

## 系统代理

设置页「通用」卡与托盘菜单均有「系统代理」开关（状态跨重启保持）：

- **开启**：把 Windows 系统代理（WinINet，`Internet Settings` 注册表键）指向本网关监听地址并广播刷新，已运行的程序立即生效；开启前的原值（ProxyEnable/ProxyServer/ProxyOverride/AutoConfigURL）快照到数据目录。
- **绕过列表追加而非覆盖**：自动补 `localhost;127.*;<local>`，用户自加的条目原样保留。
- **关闭/退出**：恢复快照原值而非简单置空——即使你之前开着别的代理（如 8890），退出后照常还原。
- **崩溃自愈**：异常退出残留的"指向本网关"的代理，会在下次启动时自动恢复原值；开关若原本开启，引擎起来后会自动重新接管（状态跨重启保持）。
- **冲突保护**：仅当"我们已接管（快照存在）且会话中被其他程序改走"时才判定为被抢占——自动放弃并关闭开关，避免与对方互相抢写；自己退出时还原的原值不会被误判。
- 引擎停止时开关自动失效（不生效）；重新运行引擎后自动恢复接管。

## 开机启动

设置页（⚙）中有「开机启动」拨动开关：开启后写入当前用户注册表 Run 键（`HKCU\Software\Microsoft\Windows\CurrentVersion\Run\failgate`，无需管理员权限），登录 Windows 时自动启动并**最小化到托盘**（`--minimized` 参数，不弹窗口，代理直接可用）；关闭开关即移除该注册表项。exe 移动位置后重新启动程序会自动修正注册表中的路径。开关状态以注册表为准，GUI 每次启动时读取真实状态。

## 自动更新

设置页「关于与更新」卡片可手动检查/下载/应用更新；「自动检查更新」开关（默认开）每 24 小时在启动时静默检查一次：

- **更新源**：GitHub Releases（仓库取自 Cargo.toml 的 `repository` 字段），仅认 latest release，预发布版本不会推送给用户
- **下载通道**：引擎运行时**优先经本网关自身转发下载**（享受上游故障切换，GitHub 资产域名不稳时也能下载），失败自动回退直连
- **完整性校验**：长度与 Content-Length 一致 + `sha256` 与 `.sha256` 资产一致 + PE 头（MZ）三重校验全部通过才落盘为 `<exe>.new`
- **原子替换**：校验通过后 `当前 exe → .old`、`.new → 当前名`（Windows 允许改名运行中的 exe），随即分离启动新版本并退出；系统代理先恢复原值，新实例启动后按持久化意图自动重新接管
- **回滚**：`.old` 保留上一版本，exe 目录只读等异常时更新中止并提示；更新失败可手动把 `.old` 改回
- 检查/下载失败均静默或轻提示，不影响代理主功能

## config.toml

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
priority = 1             # 数字越小优先级越高
# username = "u"         # 可选：上游认证（SOCKS5 RFC1929 / HTTP Basic）
# password = "p"

[[upstreams]]
name = "clash"
addr = "127.0.0.1:7890"
type = "auto"
priority = 2
```

配置查找顺序：**exe 同目录（便携模式）→ `%LOCALAPPDATA%\failgate` → 工作目录**；都不存在时自动写入 `%LOCALAPPDATA%\failgate\config.toml`（exe 位于 Program Files 等只读目录时依然可用；exe 同目录存在 config.toml 则始终优先使用，便携携带无忧）。

引擎日志同时落盘到 `%LOCALAPPDATA%\failgate\logs\failgate.log`（超过 5MB 自动轮转为 `failgate.log.1`）；设置页提供「打开日志目录」「打开配置所在目录」快捷入口。

上游认证（`username`/`password` 可选字段，留空即无认证）：

- **SOCKS5 上游**：按 RFC 1929 完成用户名/密码子协商；greeting 同时提供 `0x02`（优先）与 `0x00`（兜底）两种方法，服务器任选其一均可
- **HTTP 上游**：CONNECT 隧道与纯 HTTP 转发请求均自动携带 `Proxy-Authorization: Basic`（仅经 HTTP 型上游转发时添加，不泄漏给目标站点）
- 认证失败按上游故障处理：标记 DOWN 并自动切换到下一个上游

## 工作机制

- **协议探测**：`type = "auto"` 时对上游发 SOCKS5 握手包，回 `0x05` 判为 SOCKS5，否则判为 HTTP；结果缓存，也可手动指定类型。
- **主动健康检查**：每隔 `interval_secs` 经每个上游对 `test_url` 发一次 GET（期望 2xx），记录延迟；连续失败 `fail_threshold` 次标记 DOWN，连续成功 `success_threshold` 次恢复 UP（滞后防抖动）。启动时立即做一轮。
- **被动快速切换**：新连接拨上游失败（拒连/握手失败）→ 立即标记 DOWN，**在向客户端转发任何字节之前**沿优先级就地换下一个上游重试（对客户端透明），并触发一轮全量检查。经代理转发后才发现目标不可达（HTTP CONNECT 非 2xx、SOCKS5 错误码）不标记 DOWN，但同样切换重试。
- **路由与切回**：每次新连接选择「状态为 UP 的最高优先级上游」；高优先级恢复后新连接自动回到它。全部 DOWN 时仍按优先级尽力拨号。
- **已建立的连接不迁移**：切换只影响新连接，旧连接继续走原上游直至结束。

## 日志

日志分两类，共用底部面板（保留最近 200 条），headless 模式同时打印到控制台：

- **转发日志**：每次连接在隧道建立时记录 `→ 协议 host:port 经 [上游]`（协议为 CONNECT / GET / POST / SOCKS5 等），可在设置卡用「转发日志」开关关闭（持久化到 `general.forward_log`，默认开启）。
- **控制事件**：DOWN（⛔）/ UP（✅）/ 切换（🔀）/ 手动测试（🔍）/ 协议探测结果。

## 代码结构

```text
src/
  main.rs            入口：CLI 参数（--headless / --minimized / --updated）、GUI 与 headless 启动
  config.rs          TOML 配置模型与加载/保存（带默认值回退与单元测试）
  autostart.rs       开机启动：HKCU Run 注册表键读写 + 路径自愈
  sysproxy.rs        系统代理：WinINet 注册表 + 快照恢复/崩溃自愈/冲突保护
  update.rs          自动更新：GitHub Releases 检查、经网关优先下载、原子替换
  tray.rs            托盘图标/菜单（程序化生成三态图标，事件转发给 GUI）
  ui/
    ui.rs            App 状态与 eframe 编排（logic 托盘/关闭拦截 + 布局）
    theme.rs         明暗两套配色、Visuals 定制、中文字体加载
    widgets.rs       基础组件：卡片、胶囊徽章、拨动开关、日志着色
    cards.rs         各视图与卡片渲染：状态卡 / 上游卡 / 添加 / 独立设置页 / 日志 / 页脚
  engine/
    handle.rs        引擎生命周期（后台线程 + tokio runtime、启停控制）
    state.rs         共享状态：快照、200 条日志环、EngineCtx
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
    stream.rs        隧道流工具：PrefixedStream（前缀回放）、read_head
    url.rs           host:port / authority / 探测 URL 解析
```

## 验收命令

```bat
curl -x http://127.0.0.1:8888 https://www.google.com
curl -x http://127.0.0.1:8888 http://www.gstatic.com/generate_204
curl -x socks5h://127.0.0.1:8888 https://www.google.com
```

三者均应成功（本仓库 `test_curl.bat` 为以上测试的批处理版本）。单元测试：`cargo test`（覆盖配置解析、版本号比较、GitHub 资产筛选、sha256 解析、URL/authority 解析、HTTP 改写语义、路由候选排序、健康检查滞后转移、头部读取器等，共 45 个）。

## 构建

```bat
cargo build --release
```

产物：`target\release\failgate.exe`（自包含单文件）。

> 渲染后端说明：GUI 使用 **wgpu**（DX12/Vulkan，兼容远程桌面），之前使用的 glow（OpenGL）在部分远程会话/驱动状态下会出现窗口白屏，故已切换。
> 本项目使用 GNU 工具链构建（见 `.rustup override` 与 `.cargo/config.toml`）。本机 MSVC 链接器缺失（VS 2026 未装 C++ 工作负载），如需切回 MSVC：在 VS Installer 中为 VS 2026 勾选「使用 C++ 的桌面开发」，然后 `rustup override unset`。
> crates 镜像说明：本机 TUN 虚拟网卡环境下 cargo 到 tuna/ustc 镜像的 TLS 握手被吞，项目级 `.cargo/config.toml` 已改用阿里云镜像（仅影响本项目）。

## 已知限制

- 不代理 UDP（SOCKS5 仅支持 CONNECT，BIND/UDP ASSOCIATE 明确拒绝）
- 入站侧（8888 入口）SOCKS5 仅支持无认证（仅监听回环地址，本地使用无需认证）
- 健康检查 `test_url` 仅支持 `http://`（默认 generate_204 即为 http）
- 纯 HTTP 转发会强制 `Connection: close`（客户端每请求一条连接；浏览器/ws 均不受影响，ws 走 CONNECT）
- 默认 8s 一次的健康检查对每个上游每天约 1 万次 204 探测，量级无害；若在意可在 GUI 调大间隔
- 切换只影响新连接；SOCKS4 入站请求被拒绝（仅 SOCKS5/HTTP）
