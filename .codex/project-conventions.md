# 项目开发规范

更新时间：2026-06-21
适用范围：当前项目

## 项目概况

- 技术栈：Rust workspace，本项目主产物为 `clash-tui` 本地 TUI/CLI 控制器；TUI 使用 `ratatui` + `crossterm`，CLI 使用 `clap`。
- 项目不提供浏览器 UI、Vite/React 构建链路、SPA 静态资源服务或对外 HTTP/WS 管理 API。
- 包管理器：Rust 使用 Cargo workspace；项目任务统一通过 `cargo xtask ...` 入口执行，不再保留根 `package.json`、npm 脚本入口或 Node 运行时要求；打包、包校验和原 `.mjs` 脚本测试覆盖已迁入 Rust `xtask`，脚本目录仅保留 Bash/Python 验收工具。
- 版本语义：项目只维护两个发布相关版本号：根 `Cargo.toml` 的 `[workspace.metadata.clash-tui].app-version` 表示产品/兼容版本，打包或运行时解析的 `mihomo` version 表示内置核心版本；Cargo crate 不维护独立 `0.x` 产品版本，包 manifest 的 `versions` 只输出 `app`，`mihomo.version` 位于 manifest 顶层 `mihomo` 信息中。
- 主要入口：`crates/clash-tui/src/main.rs`；CLI 在 `crates/clash-tui/src/cli`；TUI 在 `crates/clash-tui/src/tui`。

## 目录与模块

- 原始参考：用户本机桌面端源码目录作为行为参考源；只读对照功能、策略和配置语义，不直接修改参考仓库。
- 本地 actions/services：`crates/clash-tui/src/actions` 封装 core、config、profile、runtime、subscription、system 等本地业务动作，供 CLI/TUI 共用。
- mihomo controller：`crates/clash-tui/src/mihomo_controller` 是唯一允许拼接 mihomo REST path 的 typed adapter；TUI/action 不直接拼 raw REST/WS path。
- typed controller model 必须用真实 mihomo 响应校验，不能只按 mock JSON 收窄类型；例如真实 `/rules` 会返回 `size: -1` 这类哨兵值，真实 `/connections` metadata 会返回 `sourceIP`/`destinationIP` 这类非标准 camelCase 字段，模型应使用有符号整数、serde alias 或兼容解析，避免生产 controller 数据解码失败。
- Dashboard 运行概览指标必须对齐桌面端数据源：实时上传/下载只来自 typed `/traffic` 的 `up/down`，内存只来自 typed `/memory` 的 `inuse`/`inUse`；首页不展示活跃连接数，也不应为了首页指标轮询 `/connections`。不要用 `/connections` 的 `downloadTotal`/`uploadTotal` 差值回退计算网速，也不要用 `/connections` extra 回退内存，否则会掩盖真实 stream/controller 问题。`/traffic` 与 `/memory` 是持续推送的流式响应，不能用普通 `read_to_end` 等连接关闭；`/traffic` 必须像桌面端一样由后台长期 stream 采集并维护最新值，TUI 绘制循环从进程内 snapshot 读取，不能靠 1 秒轮询 + 单帧 snapshot 碰运气。为避免终端 250ms 绘制错过短脉冲，显示层可以短暂保持最近非零 `/traffic` 样本，但不得引入 `/connections` totals 等其他来源。真实 mihomo `/memory` 首帧可能返回 `inuse=0`，应取采样窗口内的最新帧，不能把首帧 0 当最终内存。`/connections` metadata 的端口字段可能是数字或字符串，都必须兼容解析。
- 状态与运行时：`state.rs` 负责初始化共享状态；`kernel.rs` 管理 mihomo 进程；`subscriptions.rs` 管理订阅更新任务与启动 due sweep；`jobs.rs` 记录本地任务。
- `core status` 的版本号语义：Core 运行中优先使用 controller `/version`；Core 未运行、pid 缺失或 pid 已失效时，应从本地配置的 `mihomo` 二进制短超时执行 `mihomo -v` 提取版本，避免停止态显示“未知版本”。本地二进制路径不存在时才允许为空；不要为了显示版本启动 Core。
- Unix IPC path 必须随安装 home 隔离，默认位于 `<home>/ipc/mihomo.sock`；不要使用全局 `/tmp/mihomo.sock`，否则多套安装会互相污染 controller 状态。Headless 内部 controller 只走 Unix Socket，不做 TCP fallback；如果 socket 不存在、权限不对或健康检查超时，应直接暴露真实错误，方便排查，不要悄悄切到 `127.0.0.1:9097`。mihomo 自带 `external-controller` 是对外 REST 控制器，只能通过 Settings 的“外部控制器”开关显式启用，默认关闭，第一期只绑定 `127.0.0.1:<端口>`，不得作为内部控制通道或 fallback。
- 平台能力：`platform` 负责 TUN、system proxy 等平台差异；优先保持 Linux 可用，其他平台能力需要清晰返回状态/限制。
- 打包部署：主入口为 `cargo xtask package` / `cargo xtask verify-package`；实现位于 `xtask/src/package.rs` 与 `xtask/src/package_verify.rs`，只打包 CLI/TUI binary、mihomo、资源、在线 bootstrap、包内 installer 与可选 systemd unit。
- 发布包入口：安装后应提供 PATH 中的短命令（默认 `/usr/local/bin/clash-tui`），用户直接执行 `clash-tui` 进入 TUI；程序应通过安装布局推断 home/resource/mihomo，不要求用户手写长参数。

## 编码约定

- 改动优先沿用当前 Rust 模块边界；新增业务入口先落到 actions/services，再由 CLI/TUI 调用。
- 禁止重新引入本项目自有 Web 管理面参数或路由，例如 host、port、web-dir、auth-token、`/health`、`/api/*`、HTTP WebSocket 推送。例外：Settings 可以管理 mihomo 自带 `external-controller` 外部 REST 控制器开关和端口；该能力默认关闭、只绑定本机、Core 运行中应用需用户确认并重启 Core，且不能改变内部 Unix Socket 控制链路。
- 外部服务型能力默认删除或禁用；订阅 URL 导入/更新是保留例外。
- 用户可见 TUI 与 CLI help 默认使用中文。CLI help 至少要说明默认进入 TUI、各一级命令用途、全局选项和订阅导入示例；JSON 字段名可以保持稳定英文契约。
- TUI 是主产品入口，不是 CLI 的提示页集合；默认短命令 `clash-tui` 必须直接进入可操作 TUI，不要求用户记长参数。每个一级页都要有可见数据、当前状态、下一步操作和可发现快捷键；不能只提示“使用 CLI”。
- TUI 渲染必须按真实终端显示宽度处理中文/英文混排，优先使用 `unicode-width` 或 ratatui `Table`/布局约束做列宽，不用手写空格猜对齐；Settings、Profiles、Proxies、Rules、Connections、Jobs 等列表必须列头/列值/操作列稳定对齐，窄屏时降级隐藏说明列或截断。
- TUI 用户可见文字必须在渲染层统一走显示转换入口，普通文案/状态行使用 `views::layout::display_text(...)` 或 `display_line(...)`，表格 cell 使用 `table_cell_text(...)`，弹窗/帮助/底栏/详情等全局入口也必须统一处理；不要在页面里直接 `Line::from("中文：...")`、`Span::raw(raw_text)` 或手写空格拼宽度。数据层、配置层、订阅/Profile/节点原始名、过滤条件、选择 key 和 controller 请求参数必须保留原文，只在最终进入 TUI 显示前转换。
- SSH 场景下 `TERM=xterm-256color` 只能代表兼容能力，不能可靠识别真实终端客户端；TUI 必须保留“终端显示”手动切换入口，并允许 `CLASH_TUI_DISPLAY_MODE` 临时覆盖。终端显示模式负责边框、线条、滚动条等符号风格，至少覆盖标准和基础线框；修复不同终端错位时不要只靠自动识别或硬编码某个客户端名。
- TUI 主题与颜色必须走统一主题 token；标题栏、主体、底栏、面板和弹窗空白区域使用同一个全局 `canvas` 背景，不再使用标题/正文两段大面积背景。新增颜色不要在页面里散落 `Color::Rgb(...)`，应先补主题 token 和蓝色/深橙两套取值，再由 `views::layout` 的公共样式函数提供给页面、弹窗、表格、滚动条和进度条。每帧基础背景只铺 style，不要对整屏或主内容区使用 `Clear.render`；`Clear` 只用于弹窗、浮层等需要覆盖已绘制内容的区域，避免切页时产生明显闪烁。
- 特定终端里的中文标点宽度异常必须走独立“中文标点”显示设置，并允许 `CLASH_TUI_PUNCTUATION_MODE` 临时覆盖。该设置只影响 TUI 显示层，不能改变订阅名、节点名、过滤条件、Profile/runtime 内容或 controller 请求参数；至少支持保留、优化标点 `colon-comma`、常见标点三档。常见标点档负责把常见中文标点替换为英文标点；优化标点档基于常见标点，但保留已在目标终端确认正常的问号、感叹号、引号、单引、线条、破折、省略号和全角空格，同时对 `：`、`，`、`；`、`。`、`、` 替换后补一个英文空格，并吞掉原本紧跟的一个空白，避免出现 `操作:Enter` 或双空格。后续在带 block 的临时测试页确认其他异常符号后，继续追加到优化标点档；默认仍保留原中文标点。
- Proxies 这类真实订阅表格里的 emoji/旗帜图标在不同终端宽度不稳定；表格 cell 可以去掉这些图标，但列内截断交给 `Table`/`Constraint`，底层选择、过滤和 controller 调用必须继续使用原始 group/node/provider 名称。
- 规则、连接、任务、日志、总览等非 Proxies 页面也可能展示代理/策略组/Profile 名称；凡是来自订阅或 controller 的名称字段进入 TUI 表格/摘要/详情前，都应走统一稳定显示层（Table cell 用不预截断的清洗文本，普通摘要/详情行才按区域宽度截断），避免 `🚀节点选择`、旗帜或 emoji 在 Rules 代理列等位置造成 mojibake 或列宽漂移。过滤、选择、controller 调用仍使用原始值。
- TUI 可操作列表需要固定心智模型：选择光标、名称列、状态/当前值列、动作列分离；Settings 这类“配置项 + 当前值 + 操作”的页面优先用 `Table`/共享布局 helper 渲染，动作列右侧对齐，不能用散落空格让中英文自然漂移。
- TUI 长列表如果有截断/滚动，优先使用 ratatui 内置 `Scrollbar` / `ScrollbarState` 渲染可见滚动条，不要手搓滑块；选择器/表格选中态优先用 `List/ListState` 的 `highlight_style` 或 `Table/TableState` 的 `row_highlight_style`，必要时对选中行 `Rect` 整体 `set_style`，不要靠字符串补空格伪造整行背景。涉及节点名/延迟、组名/当前值这类多列内容时必须用 `Table`/`Constraint` 做列布局；`Table` cell 文本只做控制字符、空白和不稳定图标清理，不在 `Cell::from(...)` 前按猜测宽度预截断，截断和列边界交给 `Table`/`Constraint` 统一处理，尤其要避免 `：`、`，`、`；`、中文宽字符、全角/半角标点后继续用手工空格拼列。Dashboard 首页代理浮层和 `3 代理` 完整页是两条渲染路径，修正列宽/宽字符问题时必须同时检查两边，不能用首页浮层验收替代完整 Proxies 页。滚动条应画在内容区最右列，列表文本宽度预留 1 列，避免内容、滚动条和边框互相顶歪。带边框浮层需要统一边界语义：`Rect::right()` 是右开区间，浮层宽度、清理区域和滚动条区域必须共享同一右边界；如果滚动条位于浮层右侧，应渲染在右边框列或明确预留并清掉旧边框，不能出现“内层滚动条 + 外层右边框”两条竖线错位。首页代理组/节点浮层属于快速选择器，展开后至少应显示多行上下文，并用滚动条表达当前位置，不能只显示当前一行让用户盲选。
- TUI 后台刷新或 controller/runtime 数据替换不能把用户当前选择重置到顶部；Profiles/Proxies/Rules/Connections/Jobs 等列表应按稳定 key/name/id 恢复选中项，只有选中项消失或被过滤掉时才夹到可见范围。Proxies 这类同时维护原始 index 与稳定 key 的页面，用户导航过的 key 必须持续优先于可能陈旧的原始 index，不要用 10 秒这类短时效窗口让刷新后又回顶。
- Proxies 主界面固定为“代理组 / 节点”两栏模型，`f` 只在代理组与节点之间切换；Provider 不进入主表格、焦点循环或快捷键提示。底层 `proxy-providers` 解析、自动刷新、诊断和 CLI provider 命令保留；桌面式 Provider 操作走独立入口/弹窗，并且代理 Provider 列表与自动刷新候选必须按桌面端口径只展示/处理 `vehicleType` 为 `HTTP` 或 `File` 的真实 Provider，过滤 mihomo 内置 `Compatible`/默认策略组等伪 Provider，过滤后为空则不显示 `p Provider` 提示。Proxies 表格在宽终端也必须保持紧凑列宽，策略组/当前节点/节点数、节点/类型/延迟/状态等列不要用会吞掉剩余宽度的 `Min` 把后续列甩到屏幕右侧。
- TUI 换页、弹窗关闭、日志页切换和长文本刷新必须避免字符残留：必要时在 view 切换时清屏，渲染前清理区域，日志/错误文本用安全化函数处理 `\r`、`\t`、控制字符和超长行，不能让 mihomo stack trace 或 panic 原样冲破布局。
- TUI 日志页只能作为日志阅读器，不能让 mihomo panic/stack trace 淹没其他页面；长日志、panic、traceback 应在日志页内按宽度截断/滚动/清屏，普通状态栏只显示脱敏摘要。修复字符残留后必须用 TestBackend 或真实 PTY 切页回归确认上一页长内容不会残留到下一页。
- TUI 关键操作结果不能被后台刷新立刻覆盖：订阅导入、设置应用、日志清空、诊断导出等成功/失败状态应短时间 pin/protect；Core/job/kernel 状态事件只能更新数据，不应让用户刚看到的操作结果瞬间消失。
- Jobs 取消语义：当前进程内注册了 abort handle 的 Pending/Running 任务可以被真实取消，并标记为 `Cancelled`；历史记录、跨进程任务或缺少执行句柄的任务必须返回 `supported=false` 与清晰中文 message，不能伪装为已终止。历史加载时发现 Pending/Running 任务应标记为已中断/已取消，避免陈旧任务阻塞后续订阅更新。
- TUI 任务详情、订阅错误、controller 错误等长文本应通过弹窗或受控区域展示，关闭键使用 Esc/Enter；所有行都要经过 URL/控制字符脱敏和宽度截断，不能把 JSON key 中的完整订阅 URL、mihomo stack trace 或超长错误直接打到主界面。
- TUI 长状态、订阅失败、provider/controller 错误和诊断摘要不能只依赖底栏一行；必须有可发现的完整查看入口。当前全局约定为 `n` 打开“消息历史”，显示当前状态与最近状态；该全局键不能抢占输入框文本输入或确认框 `n` 取消语义。
- TUI 状态栏若同时包含数量、后续快捷键和一段动态长建议，数量/快捷键提示必须放在长文本之前，避免终端宽度截断后只剩第一条建议而看不到“按 n 查看”等可发现入口；例如诊断建议使用 `建议（共N条，按 n 查看）：...`，不要把计数追加到建议末尾。
- TUI 底栏、确认弹窗、输入弹窗、详情弹窗和错误弹窗属于全局渲染入口；这些入口必须统一走 URL 脱敏、控制字符清理和显示宽度截断，不能直接渲染 raw status/prompt/detail line。
- 真实订阅导入验收优先使用 `profile import-url --stdin --start-core` 或 TUI 粘贴输入，避免订阅 URL 进入 shell history；验收必须继续检查 runtime 生成、current profile 切换/刷新和 `proxy groups` 非空，不能只看 import/update/status/job 成功；远程订阅下载结果如果只有空 `proxies: []` 或空 `proxy-providers: {}`，应在导入/更新阶段失败，不能保存成“成功但空代理”的 profile；节点订阅/base64 转换至少覆盖 `vmess`、`ss`、`ssr`、`trojan`、`vless`、`hysteria2`/`hy2`、`hysteria`/`hy`、`tuic`、`anytls`、`http`/`https`、`socks`/`socks5`、`wireguard`/`wg`；混合节点订阅中少量未知 scheme 或单行解析失败应跳过并保留可用节点，全部 URI 都不可转换时必须失败且错误原因可见；CLI 成功输出只应返回脱敏摘要，错误输出必须隐藏 `http://`/`https://` URL，JSON object key 中的 URL 也必须脱敏。
- Profile 切换需要遵守桌面端策略：忙锁、30 秒超时、验证失败/错误/超时回滚。Profile 切换、当前订阅更新、删除当前订阅导致的 runtime 内容变化必须先生成 runtime、再用本地 mihomo 二进制 `-t` 校验，Core/外部 Core 运行中只通过内部 Unix Socket `PUT /configs` 热加载；reload 失败直接回滚并报错，不 fallback restart。TUN、外部控制器、核心日志、Core start/stop/restart 等会影响启动参数或进程生命周期的场景继续使用 restart/start/stop 语义，不混入 Profile runtime reload 链路。
- 远程订阅导入和订阅更新需要遵守三段重试：direct -> Clash proxy -> system proxy；TUI/CLI 导入成功输出应展示实际成功策略但继续脱敏 URL；TUI 默认进入或显式 `tui` 启动时只执行一次订阅 due sweep，到期才入队更新，不保留后台定时器/周期 tick；Core 未启动也允许下载保存订阅；一次性 CLI 命令不隐式更新订阅，只有显式 `subscription update` / `subscription update --due` 才更新。失败原因不得把订阅 URL 写入 job history。
- 远程订阅下载默认 User-Agent 要保持桌面端兼容口径：`clash-verge/v2.5.1`。这是订阅服务兼容/风控相关字段，不跟随本项目产品名改成 `clash-tui`；只有用户在 Profile option 中显式设置 `user_agent` 时才覆盖默认值。
- Runtime 端口输出跟随桌面端开关语义：默认只保留 `mixed-port`；`socks-port`、`port`、`redir-port`、`tproxy-port` 只有对应 `socks_enabled`、`http_enabled`、`redir_enabled`、`tproxy_enabled` 显式开启时才写入 runtime，避免未启用透明代理也占用 7895/7896 造成 Core 启动失败。
- Settings P0 至少覆盖 IPv6、Allow LAN、Unified Delay、Log Level、Mixed Port、mihomo 外部控制器开关/端口、TUN、system proxy、DNS 开关；DNS 覆写失败要回滚。外部控制器显示以 Core 运行态 `/configs` 为准，Core 未运行才显示本地配置；安全的本机运行态可以同步回配置，`0.0.0.0` 等非本机绑定只显示告警，不静默保存。
- 敏感信息、生产订阅 URL、生产 token 和外部凭证不得写入 `.task`、日志或文档；用户明确声明为本地/内网 `dev-only` 的测试订阅 URL 可以写入测试需求，并标明用途与环境。

## 常用命令

- 格式化检查：`cargo fmt --all --check`
- Clash TUI 检查：`cargo check -p clash-tui`
- Clash TUI 测试：`cargo test -p clash-tui`
- 完整本地 CI：`cargo xtask ci`
- 供应链检查：`cargo xtask policy-check`
- 打包：`cargo xtask package --no-docker`
- 包校验：`cargo xtask verify-package --archive <tar.gz> --manifest <manifest.json>`
- 远端 TUI smoke：`cargo xtask tui-remote-smoke --host <host> --user <user> --bin clash-tui --rows 40 --cols 140 --output <report.json>`
- 旧 npm/Node 脚本入口已移除；不要使用 `npm run ...`、`node scripts/...` 或新增 `.mjs` 作为项目任务入口。

## 脚本目录与验收脚本

- 当前脚本位置仍以现有仓库事实为准：远端 TUI smoke 在 `scripts/clash-tui-remote-smoke.sh`，GNOME/TUN 验收脚本和 Python 测试仍位于 `scripts/` 根目录；打包、包校验和原 Node 脚本测试已迁入 `xtask/src/`。
- 后续如果整理目录，建议一次性迁移为专用分组，而不是只移动单个脚本：
  - `xtask/src/`：`package.rs`、`package_verify.rs`、`script_tests.rs`。
  - `scripts/acceptance/`：`clash-tui-remote-smoke.sh`、`clash-tui-system-proxy-gnome-acceptance.sh`、`clash-tui-system-proxy-gnome-smoke.py`、`clash-tui-tun-linux-smoke.py`。
  - `scripts/tests/`：`test_headless_*.py`。
- 脚本目录迁移必须作为独立提交处理，不能和功能改动混在一起；迁移时同步更新 `cargo xtask` 入口、`xtask/src/package.rs` 的包内复制路径、`xtask/src/package_verify.rs` 的 marker 检查、测试文件中的脚本路径、README/packaging README、`.codex/project-conventions.md` 和相关 `.task/.../PROGRESS.md`。
- 脚本目录迁移后的最小验证：`cargo test -p xtask`、`bash -n <所有 .sh>`、`python3 -m py_compile <所有 .py>`、`python3 -m unittest discover -s scripts -p 'test_*.py'`，再跑一次纯拷贝 package smoke 或真实 Buildx package verifier，确认包内工具仍被复制且 marker 校验通过。
- 不要把 `target/` 下的 JSON/raw/clean 验收证据、`.task/` 任务记录、远端临时 wrapper、订阅 URL、token 或 SSH 密码提交进 Git。

## 验收环境

- Linux 特权验收机：`192.168.4.77`，SSH `root/root`；仅限内网临时验收（dev-only/test-only），用于 TUN/system proxy/package 等 Linux 能力验证，不得记录生产凭证或外部订阅 URL。
- GNOME 桌面验收机：`192.168.4.84`，SSH `haokejie/123456`；仅限内网临时验收（dev-only/test-only），用于桌面会话、GNOME system proxy 和普通用户 TUI/CLI 验收。该凭据不是生产凭证，任务结束建议改密或删除。
- 当前内网 `dev-only/internal-test` 订阅验收用例允许完整记录并用于 import/update/status/proxy groups 测试：
  - `https://api3.nimenshishangdi.cc/dazhutou/c49df71acdb8633f974b63e49856915d`
  - `https://dash.pqjc.site/api/v1/pq/2ac0b5057a222bc5bf08ed2b9703ca2a`
  - `https://v2.missblog.net/api/v1/client/subscribe?token=5afc6b196f1a1c1b839aa7bcfe57d9d2`
- 真实 Linux 成品验收口径：本地使用 Docker Buildx 构建 `linux/amd64` / `x86_64-unknown-linux-gnu` 成品包，打包时使用真实 mihomo 和真实 geo 资源；通过 `scp/ssh` 上传到验收机运行，不在验收机上使用 Docker、Node、Rust/Cargo 或源码构建。
- TUI、首页、主题、键盘交互或布局类改动的固定验收矩阵：先跑本地 `cargo fmt --all --check`、`cargo check -p clash-tui`、相关 `cargo test -p clash-tui <filter>` 或全量 crate 测试；再用 Buildx 构建 linux/amd64 真包，并用 `cargo xtask verify-package --archive <tar.gz> --manifest <manifest.json> --expect-commit <HEAD>` 校验包 SHA、manifest gitCommit、二进制/资源 SHA 和包内工具标记；随后部署同一个成品包到 `192.168.4.84` 与 `192.168.4.77` 的 正式路径，短命令必须是 `/usr/local/bin/clash-tui`；两台都要跑 `clash-tui --json settings show` 和中文 `--help` smoke；最后两台都必须跑 `cargo xtask tui-remote-smoke --host <host> --user <user> --bin clash-tui --rows 40 --cols 140 --output target/<name>.json`，JSON 必须 `ok=true`、`expectRc=0`、`missing=[]`，markers 至少包含 title/dashboard/运行概览/代理选择/快速开关/模式切换/代理组浮层/底栏 `…更多`/TUI_EXITED/TRACE_BEGIN/TRACE_END，trace 至少 `char>=4`、`esc>=1`；当前脚本只发送稳定 TTY UI 序列 `2,1,g,Esc,q`，不在 smoke 中触发 `r` 刷新，避免远端 controller/runtime 刷新波动把键位验收拖成假失败；刷新、订阅更新和 controller 数据链路用专门 CLI/TUN/订阅验收覆盖。只有本地 PTY smoke、临时 expect 或单台服务器 smoke 不能作为阶段完成依据。
- 上述 TUI 远端 smoke 结果如果一台机器无订阅数据，只能证明首页空态和基础交互；涉及订阅流量、代理组/节点数据、导入/更新或代理选择闭环时，必须在有真实订阅数据的验收机上额外确认，或使用用户授权的 dev-only/internal-test 订阅重新导入并检查 `proxy groups` 非空。验收证据写入 ignored 的 `target/` 与 `.task/.../PROGRESS.md`，不要提交 raw PTY log、订阅 URL、token 或密码。
- TUI/首页/主题类改动的详细测试步骤：
  1. 本地静态与单测：运行 `cargo fmt --all --check`、`cargo check -p clash-tui`、相关 `cargo test -p clash-tui <filter>`；如果改动影响共享 TUI 布局、状态、按键或 view 切换，跑 `cargo test -p clash-tui tui::tests` 或全量 `cargo test -p clash-tui`。
  2. 验收脚本自检：运行 `bash -n scripts/clash-tui-remote-smoke.sh`、`cargo test -p xtask remote_tui_smoke_script_keeps_required_acceptance_safeguards`、`git diff --check`。
  3. Buildx 真包：先取 `HEAD_COMMIT="$(git rev-parse HEAD)"`，再用缓存和显式源构建，例如 `CLASH_TUI_PACKAGE_CARGO_CACHE_DIR=target/clash-tui-cargo-cache CLASH_TUI_PACKAGE_DOWNLOAD_CACHE_DIR=target/clash-tui-download-cache CLASH_TUI_PACKAGE_DEBIAN_MIRROR=http://deb.debian.org/debian CLASH_TUI_PACKAGE_DEBIAN_SECURITY_MIRROR=http://deb.debian.org/debian-security CLASH_TUI_PACKAGE_RUSTUP_DIST_SERVER=https://static.rust-lang.org cargo xtask package --target x86_64-unknown-linux-gnu`。
  4. 包校验：运行 `cargo xtask verify-package --archive target/clash-tui-dist/clash-tui-linux-x86_64.tar.gz --manifest target/clash-tui-dist/clash-tui-linux-x86_64.manifest.json --expect-commit "$HEAD_COMMIT" --json`；结果必须 `ok=true`，并记录 archive SHA、manifest gitCommit、headless SHA、mihomo SHA。
  5. 部署 4.84：上传同一个 tar.gz 与 `packaging/clash-tui/install.sh` 到 `192.168.4.84`，使用 sudo 安装到 `/opt/clash-tui`、`/etc/clash-tui`、`/var/lib/clash-tui`、service `clash-tui.service`、短命令 `clash-tui`；普通用户和 root systemd 共用同一 home 时，不能把 owner 递归改成普通用户，因为 unit 的 `CapabilityBoundingSet` 不含 `CAP_DAC_OVERRIDE`，root 服务会无法写 geo/runtime/log。推荐 `chown -R root:haokejie /var/lib/clash-tui`，目录 `chmod g+rwX`，文件按需 `chmod g+rw`，或使用等价 ACL。
  6. 部署 4.77：上传同一个 tar.gz 与 installer 到 `192.168.4.77`，root 安装到同样 正式路径和短命令；不要在服务器上构建源码，不要把 release 包安装到 `/usr/local/bin` 作为 prefix，`/usr/local/bin` 只作为 `--bin-dir`。
  7. 双机 CLI smoke：两台都运行 `clash-tui --json settings show >/tmp/<name>-settings.json`，再运行 `clash-tui --help | sed -n '1,40p'`，确认 help 是中文并包含 `tui/core/profile/proxy/settings/tun/system-proxy` 等命令。
  8. 双机 TUI smoke：4.84 使用普通用户，4.77 使用 root，分别运行 `CLASH_TUI_SSH_PASSWORD=<本地测试密码> cargo xtask tui-remote-smoke --host 192.168.4.84 --user haokejie --bin clash-tui --rows 40 --cols 140 --output target/clash-tui-remote-smoke-<commit>-192.168.4.84.json` 与 `CLASH_TUI_SSH_PASSWORD=<本地测试密码> cargo xtask tui-remote-smoke --host 192.168.4.77 --user root --bin clash-tui --rows 40 --cols 140 --output target/clash-tui-remote-smoke-<commit>-192.168.4.77.json`。
  9. JSON 判定：用 `python3 -m json.tool target/*.json` 或等价命令复核；两份报告都必须 `ok=true`、`expectRc=0`、`missing=[]`、trace 至少 `char>=4 esc>=1`，当前脚本通常为 `char=4 esc=1 total=5`；markers 必须覆盖首页、代理选择、快速开关、模式切换、代理组浮层、底栏 `…更多`、TUI 正常退出和 trace 起止。
  10. 记录证据：把命令、包 SHA、两台安装路径、CLI smoke、两份 JSON 报告路径和差异写入当前 `.task/.../PROGRESS.md`；`.task` 与 `target` 都保持 ignored，不提交验收原始日志。
- 真实订阅导入后首次 `core status` 可能短暂显示 `unhealthy`，需要等待 Unix socket/controller 就绪后再用 `proxy groups` 判定代理组是否加载成功。
- TUI 订阅导入激活后，如果 runtime 使用 `proxy-providers`，导入等待阶段应主动尝试一次 typed provider update，并使用更长的代理组就绪窗口；provider 自动刷新优先 runtime `proxy-groups[].use` 引用的 Provider，再回退到 runtime provider keys；普通直接节点订阅不应在页面刷新时隐式触发 provider 网络请求。
- 代理列表为空时 TUI 应自动保留最近诊断摘要，并支持按 `D` 手动重新诊断；Proxies 空列表页必须直接展示 runtime 配置计数、代理类型、代理/Provider/策略组样本与策略组引用数量，不能只显示一句空列表；当 runtime 已有策略组但 controller 未就绪时，TUI 应显示 runtime 离线预览，让用户看到已解析出的策略组/节点，并允许在 Dashboard 与 Proxies 中将节点保存为“离线预选”，写入当前 Profile `selected`，Core/controller 就绪后自动应用；离线预选必须校验 runtime 里存在目标组/节点，且 provider-backed 订阅要展开 `proxy-providers[].path` 指向的本地 provider cache，不能只看 `proxy-groups[].use` 引用名；mihomo controller 可能返回 runtime 中没有显式 `proxy-groups: GLOBAL` 的虚拟 `GLOBAL` 策略组，runtime 有 `proxies` 节点但没有显式 `GLOBAL` 时，应合成可预选的 `GLOBAL` 离线预览；离线预选不能显示为已切换成功，也不能直接调用 mihomo REST；订阅导入激活后如果代理组等待超时，TUI 必须自动保存脱敏诊断快照并在状态栏显示路径；需要手动保存复现证据时在 TUI 按 `E` 导出脱敏诊断快照，或运行 `clash-tui --json diagnose --save` 写入 `<home>/diagnostics/diagnose-*.json`；必要时再运行 `clash-tui --json diagnose` 收集 current profile、Core、controller、runtime、proxy groups、subscription status、脱敏 Core 日志摘要和建议；不要只凭 TUI 空列表判断是订阅解析、runtime 生成还是 controller 加载问题。
- 验收脚本不要把自定义 CLI 输出写入应用保留文件名，例如 `$HOME/jobs.json`；`jobs.json` 是应用 job history，脚本输出应使用 `jobs-cli.json`、`status-cli.json` 等独立文件名，否则会污染下一次短命令初始化。
- CLI `--json` 输出通常包在 `{message,data}` 下；验收脚本必须先解包 `data` 再判断业务字段。当前契约示例：`profile list` 使用 `data.current` 与 `data.items`，`proxy groups` 返回 mihomo 风格的 `data.proxies` 字典（策略组和节点都是 map value，不能按顶层 `groups` 列表统计），`subscription update <uid>` 的最终任务在 `data.job`，runtime 刷新结果在 `data.job.result`。不要因为脚本按错字段读到 0 就误判“订阅导入成功但代理为空”。
- 远端复杂 smoke/验收脚本不要直接嵌套 `ssh 'python3 - <<PY'`、多层 heredoc、管道或正则字符串；已有多次本地 shell 引号截断事故。脚本超过一行、包含单双引号混用、heredoc、管道、正则、JSON、变量展开、PTY/expect 控制或需要复用结果时，统一先在本地 `mktemp` 写完整脚本，必要时本地语法检查，再 `scp` 上传验收机执行并删除远端临时脚本；远端部署、sudo、TUI TTY、离线预选闭环这类验收也必须上传 wrapper/script 再执行，不用 inline `ssh bash -lc <长命令>` 充当阶段验收；复杂脱敏/正则/JSON 处理优先放到本地 Python 或独立脚本，不塞进远端内联 `sed`；只有很短、无复杂引号的单行命令可以直接写进 ssh。
- 停止态 `core status` 验收必须先证明 正式 service 和 mihomo 真的停止：root 机可直接上传脚本执行，普通用户机如 4.84 需通过 sudo/root 停 `clash-tui.service` 并清理 mihomo pid，再查询 `core status`；如果 `core stop` 返回权限错误、超时，或状态仍为 `running`/`unhealthy`，该结果不是停止态验收，不能拿来判断版本号是否修复。脚本必须在结束时按初始状态恢复 service/Core。
- macOS/BSD `mktemp` 模板中的 `X` 必须放在模板末尾，例如 `/tmp/clash-tui-script.XXXXXX`；不要写 `/tmp/clash-tui-script.XXXXXX.sh`，否则会生成固定文件名并导致第二次运行冲突。
- zsh 脚本函数里不要把局部变量命名为 `path`；`path` 是 zsh 绑定到 `PATH` 的特殊数组变量，误用会改写命令搜索路径并导致 `cat`、`ssh` 等基础命令突然 `command not found`。
- `expect` 使用 Tcl 语法，远端命令字符串里的 `[c]` 会被当作 Tcl 命令替换，`$?` 会被当作 Tcl 变量读取；在 `spawn ssh "..."` 里避免使用方括号进程匹配和未转义 `$`，复杂命令继续落远端脚本执行。`expect` 的正则如果包含 `[]` 字符类，不要放在双引号字符串里，优先用 braced pattern，例如 `-re {密码}` 与 `-re {(?i)(password|passphrase)}`，避免 Tcl 先做命令替换。
- 需要用 stdin 给 expect 传脚本并同时传 argv 时，必须使用 `expect -f - -- <args>`；不要写 `expect - <args>`，否则第一个参数会被当成 expect 脚本路径，可能把 tar.gz 等二进制文件当脚本读取。
- expect 自动输入 sudo/SSH 密码时不能只匹配英文 `password`/`passphrase`；中文系统的 sudo 提示可能是 `密码：`，pattern 至少要覆盖 `(?i)(password|passphrase)` 和 `密码`，否则会卡到 timeout。
- 远端 sudo 验收/部署不要用非 TTY `sudo -S` 串联多个 sudo 命令；实测 4.84 中文 sudo 提示下可能在终端输出中回显临时密码。需要 sudo 时优先上传 wrapper，用 `ssh -tt` 进入 TTY，并用单次 `sudo sh -c '...'` 执行整段 root 操作；expect 发送 sudo/SSH 密码的窗口必须 `log_user 0`，结束后再恢复日志。
- `expect { ... }` 块中每个 pattern 都必须有显式 action，例如 `eof {}`、`timeout { ... }`；不要写裸 `eof` 后接 `timeout {}`，否则 TUI 已正常退出后 expect 仍会把 `timeout` 当 Tcl 命令执行并误报失败。
- 不要用 `expect -n` 当语法检查；本机 expect 会直接执行脚本，可能在缺少参数时误开远端 shell。Expect 脚本只能做有限静态检查/人工复核，执行前用小范围真实命令验证。
- `expect` 包装的 SSH 不会自动把本地重定向 stdin 转发给远端命令；不要写 `expect_ssh ... 'bash -s' < local-script`。多行远端探测/验收脚本必须先 `scp` 上传 wrapper/script，再执行远端文件路径；只有明确在 expect 脚本里 `send` 内容时才依赖交互输入。
- TUI/expect 远端验收不要用 `ssh -- bash -lc <长命令>` 传递清理、stty、TERM、trace、启动 TUI 等多段命令；SSH/远端 shell 会重新拼接参数，容易把命令截断成 `bash -lc rm`。这类场景统一先上传远端 wrapper 脚本，再让 expect 执行该脚本路径。
- 远端临时文件清理也遵守同一规则：如果清理列表较长、包含多路径或需要和 expect/ssh 配合，不要写成 `expect -c 'spawn ssh ... rm -f ...'` 的长行；先上传短 cleanup 脚本再执行，避免 EOF 等待或参数重组导致本地 expect/ssh 悬挂。
- 只要涉及 SSH 密码/expect，即使是单条远端清理命令，也优先上传 wrapper 执行；本地内联 `expect -c` 曾卡在密码提示且 timeout 未生效，最终需要 kill 本地 expect/ssh 后重建 wrapper 清理。
- 远端 smoke 需要用 `nobody` 等非 root 用户时，不要假设组名是 `nogroup`；不同发行版可能只有 `nobody` 组。临时目录授权优先用 `chown -R nobody <path>`，或先用 `id -gn nobody` 探测真实组名。
- 执行中遇到可复用的坑、验证口径变化或重复性经验，应当当轮固化：任务相关经验写入 `.task/GOAL_MEMORY.md` 与当前 `.task/.../PROGRESS.md`；稳定项目规则同步到 `.codex/project-conventions.md`。固化内容保持脱敏、简短、可执行；除用户明确要求记录的 `dev-only/test-only` 用例外，不写生产 URL、token、原始敏感日志或客户数据。
- CentOS/RHEL SELinux Enforcing 下，从 `/tmp` 解压再复制到 `/opt` 可能保留 `user_tmp_t` 上下文，systemd 会报 `Unable to locate executable ... Permission denied`，即使文件 mode 是 `755`。安装脚本复制 prefix 和写入 systemd unit 后必须在存在 `restorecon` 时执行 `restorecon -R "$PREFIX"` 与 `restorecon -R "$SERVICE_PATH"`；验收时看到 `user_tmp_t` 先修上下文，不要用 chmod 误判。
- 正式 service 使用 root 但限制了 `CapabilityBoundingSet`，不要假设它具备完整 root DAC 覆盖能力。若为了普通用户 TUI/CLI 可写而把 `/var/lib/clash-tui` owner 改成普通用户，service 可能无法复制 geo 资源、写 runtime、写日志或创建 socket；共享 home 用 root 作为 owner，再给普通用户组写或 ACL。
- 验收机安装使用隔离路径，例如 `/opt/clash-tui-acceptance`、`/etc/clash-tui-acceptance`、`/var/lib/clash-tui-acceptance` 和 `clash-tui-acceptance.service`；先用 `--no-enable --no-start` 安装，再执行 CLI/core/system-proxy/TUN smoke。
- 隔离路径验收也应生成短命令，例如 `--bin-name clash-tui`；验证时优先运行短命令 `clash-tui --json settings show`、`clash-tui core status`，不要用一串 `--home-dir/--resource-dir/--mihomo-bin` 掩盖安装入口问题。
- 远端 sudo/root wrapper 中不要假设 `/usr/local/bin` 一定在 `PATH`；安装后的 CLI smoke 可直接调用 `/usr/local/bin/clash-tui`，同时再验证普通 shell 下短命令是否可用。
- 正式包默认安装前缀必须是 `/opt/clash-tui`；`install.sh --prefix` 是应用安装目录，不是 `/usr/local/bin`。短命令通过 `--bin-dir /usr/local/bin` 与 `--bin-name` 控制。写错前缀时只能删除与当前包逐字节一致的误写文件/目录，禁止粗暴清理 `/usr/local`。
- 远端包如果为了区分版本改名，`.sha256` 文件里的文件名也必须同步改为远端实际 basename；不要用本地原始包名校验已经改名的远端文件。
- 成品包验收优先使用 `cargo xtask verify-package --archive <tar.gz> --manifest <sidecar-manifest>` 校验 archive、sidecar manifest、tar 内 manifest、包内文件、二进制/资源 SHA、工具可执行权限，以及 GNOME/TUN smoke 工具的关键入口标记（如 `--verify-acceptance-dir`、`gnome-acceptance-verified.json`、`--verify-report`、`reportHashes`、`resolvedPath`、`CLASH_TUI_TUN_SMOKE`）。tar 内 `manifest.json` 不要求包含最终 archive SHA；tar 归档完成后才能计算自身 SHA，因此 archive SHA 以 sidecar manifest 与 `.tar.gz.sha256` 为准，远端 wrapper 不要把 tar 内缺少 `archive` 字段当失败。
- `core start/stop` 可在验收机使用真实包内 mihomo 验证；`system-proxy on/off` 和 `tun on/off` 属于会影响宿主机网络/桌面代理的操作，执行前需明确风险和恢复命令。当前验收机缺少 `org.gnome.system.proxy` schema，system proxy 只能验证 `platformApplied=false` 时配置回滚和错误可见，不能证明 GNOME 桌面成功应用；TUN 已在 root + `/dev/net/tun` 环境验证可创建 `Meta` 网卡和 `198.18.0.0/30` 路由，关闭后应确认 Core stopped 且无 mihomo/headless 残留。
- system proxy 自动应用失败时必须回滚 `enable_system_proxy` 配置，并在 JSON `manualAction` 和 TUI 消息中给出可执行手动建议：HTTP/HTTPS/SOCKS 主机、端口、忽略主机，以及 GNOME `org.gnome.system.proxy`/`gsettings` 处理方向；建议文案不要包含 `http://`/`https://`，避免和订阅 URL 泄漏检查互相干扰。
- Linux system proxy 自动应用 readiness 必须同时满足 `gsettings` 存在、`org.gnome.system.proxy` schema 存在、`DBUS_SESSION_BUS_ADDRESS` 存在，以及至少一个桌面/显示会话标记（`DISPLAY`、`WAYLAND_DISPLAY`、`XDG_CURRENT_DESKTOP` 或 `DESKTOP_SESSION`）；不要把单独的 DBus、单独的 DISPLAY、单独的 XDG/DESKTOP 变量判定为可自动应用，避免 SSH、sudo/root 或半桌面环境误写代理。
- GNOME system proxy smoke/preflight 的 JSON 报告必须保留机器可读 `binary`、`desktopSession` 和 `nextSteps`：`binary` 记录解析后的二进制路径和 SHA256，`desktopSession` 记录 DBus 与 DISPLAY/WAYLAND/XDG/DESKTOP 标记是否满足真实桌面会话条件；预检通过时给确认式 smoke 命令，环境不满足时指向桌面用户、GNOME schema、doctor 修复或输出路径修复；报告仍需保持 `mutated` 和 `urlLeak` 字段。项目主目标仍是无桌面 headless TUI/CLI，但 system proxy 的 GNOME 自动应用能力若作为最终交付能力声明，必须在真实 GNOME 桌面用户会话跑通确认式 on/off；无桌面服务器只能证明只读预检、安全拒绝/回滚和可操作手动建议，不能替代桌面成功验收。确认式 smoke 产出的 JSON 还应能通过包内 `--verify-report` 只读校验：非 root 桌面会话、`system-proxy on/off` 成功、GNOME on/off 值、restore 成功和无 URL 泄漏都必须被机器检查。包内 `clash-tui-system-proxy-gnome-acceptance.sh` 是推荐验收入口：默认只跑 preflight，只有传 `--yes` 才运行确认式 smoke、自动 verify-report，并生成目录级 `gnome-acceptance-verified.json`；完整复核通过后应生成 `gnome-acceptance-SHA256SUMS.txt`，可选 `--archive` 只打包四份 JSON 报告和 SHA 清单；`--verify-acceptance-dir` 必须保持只读，用于复核 preflight/smoke/verified 三份 JSON 是否共同证明同一次桌面成功验收，最终报告还要包含三份源报告 SHA256，方便归档证据核对。
- TUN doctor 必须覆盖 `dev-net-tun`、`privilege`、`capability-tools`、`iproute2` 检查项；非 root 场景要明确 `getcap/setcap` 是否可用，manualAction 应提示安装 libcap 工具、`setcap cap_net_admin=+ep <mihomo>`、`tun off` 和 `core stop`，且不得包含 URL scheme。
- TUN 真实验收不能只看 `core start` 立即返回或瞬时 `core status`；mihomo 刚启动时可能短暂 `unhealthy`。判定 TUN 成功必须等待并同时确认 runtime `tun.enable=true`、`Meta` link 出现、`198.18.0.0/30` route 出现；关闭后必须确认 `Meta` link/route 消失、Core stopped、无残留进程。
- Linux TUN smoke/preflight 工具必须默认只读；确认式 smoke 需要显式确认，并要求测试前 Core 停止、TUN 关闭、已有 current Profile、`Meta` link 和 `198.18.0.0/30` route 均不存在。JSON 报告需包含 `nextSteps`、`mutated`、`urlLeak` 与 cleanup 证据，方便归档 TUN on/off 真实验收。
- TUI 自动化 smoke 使用 SSH TTY 时必须显式设置终端尺寸和 TERM，例如 `ssh -tt ... 'stty rows 40 cols 140; export TERM=xterm-256color; clash-tui'`。未设置时测试机可能返回 `stty size` 为 `0 0`、`TERM=dumb`，ratatui 只输出 alternate-screen 控制序列，不能作为有效界面验证。
- 远端成品包 TUI smoke 优先使用仓库脚本 `cargo xtask tui-remote-smoke --host <host> --user <user> --bin clash-tui --rows 40 --cols 140 --output <report.json>`。该脚本会上传远端 wrapper、使用真实 `ssh -tt`、设置 `TERM`/TTY 尺寸、启用 `CLASH_TUI_TUI_INPUT_TRACE`、保存 raw/clean log 和 JSON markers；不要再用一次性临时 expect 当作阶段验收依据。
- TUI/crossterm 输入 smoke 不要用 GNU `timeout` 包住 TUI 进程；真实验收中发现 `timeout 10s clash-tui` 会导致自动化发送的按键不被 crossterm 消费。需要自动退出时由 `expect` 控制会话并发送 `q`/Ctrl-C 收尾。
- expect 捕获 TUI raw log 时，不要以为 `log_user 0` 会“只静默 stdout 但继续写 log_file”；本轮实测会导致文件日志缺少 TUI 输出。正确做法是让 expect 正常输出，并由外层重定向 stdout/stderr 到 raw log；发送 SSH 密码的短窗口内再局部 `log_user 0`，避免密码进入日志。
- TUI 自动化发送普通按键前，必须等待 alternate screen 控制序列 `ESC[?1049h` 或稳定页面标记出现，再发送 `1-8/q` 等按键；不要只在 SSH 登录后 sleep 固定时间。必要时设置 `CLASH_TUI_TUI_INPUT_TRACE`，用 trace 中的 `key code=... source=raw` 证明应用实际消费了输入。
- 使用 expect 等待 TUI ready 时不要只依赖易写错的 ESC 序列 pattern；Tcl/expect 对 `[`、`?`、`\x1b` 的转义很容易写成永不匹配。优先等待稳定可见标题（例如 `clash-tui`）或页面标记，再用 input trace 与 screen reconstruction 证明真实消费和最终画面。
- TUI 自动化检查中文状态时，原始 PTY 输出会在每个字符间夹 ANSI 光标控制序列，不能用连续中文字符串直接匹配；应先重建/清理 ANSI 画面，或用允许控制序列穿插的正则，并继续用 CLI JSON/trace 做最终事实对照。
- expect/SSH 保存的 PTY raw log 可能同时包含 ANSI 光标控制、分段中文和非预期编码呈现；不要用简单 UTF-8 中文 grep 作为最终验收判据。TUI 验收优先结合 input trace、CLI JSON、screen reconstruction 和人工可见关键画面记录。
- 自制 screen reconstruction 在中文宽字符、光标清行和边框重绘下可能漏掉标题或多行详情；远端 TUI smoke 需要同时检查去 ANSI 后的 raw PTY 输出与 input trace，不要只依赖单一屏幕模型判断“消息历史/诊断建议”是否可见。
- TUI Rules/Connections/Jobs 等表格页的验收不能只看页面能切换；需要在真实 Core/controller 数据下同时检查列头、宽字符对齐、过滤/滚动入口和错误摘要。若页面显示“解码失败”或空列表，优先抓取对应 typed controller 的原始 Unix socket JSON 形状定位模型问题。
- TUI Connections 验收需要制造真实活动连接，不能只看空表；可在验收机启动本地慢速 HTTP server，再通过 mixed-port 代理访问 `127.0.0.1:<port>` 保持连接，先等 mixed-port 监听成功，再进 TUI 验证详情弹窗、关闭选中连接和关闭后 CLI `connections list` 归零。
- TUI Dashboard 指标验收必须制造经过 mihomo 的真实流量，不能只看 TUI 自身刷新：先确认 Core 运行且 mixed-port/TUN/system proxy 至少一种接管方式生效，再通过代理或 TUN 直连访问本地/外部测试地址保持连接；打开 TUI 后至少观察数秒，确认实时下载/上传由后台 `/traffic` stream 推送并能在不按 `r` 的情况下变化，内存来自 `/memory` 且显示为非未知值。无接管方式、无代理流量或请求过快结束时，速度为 0 只能证明当前没有流量，不能判定指标坏了；需要验证上传时可在 TUN 生效环境使用 `env -u http_proxy -u https_proxy -u all_proxy -u HTTP_PROXY -u HTTPS_PROXY -u ALL_PROXY sh -c 'dd if=/dev/zero bs=1M count=64 2>/dev/null | curl -sS --max-time 30 -o /dev/null -X POST --data-binary @- https://speed.cloudflare.com/__up'` 这类命令制造上行。活跃连接只在 `7 连接` 页面验收，不作为首页指标。
- TUI 已启用 bracketed paste 并处理 `Event::Paste`；订阅链接粘贴导入的人工验收优先在任意页面直接粘贴 http(s) URL，TUI 应切到 Profiles 导入输入并预填链接，再按 Enter 应用；Profiles 页仍保留 `i` 手动输入 URL。
- TUI 验收不能只看首屏能渲染或能按数字切页；必须覆盖至少一条真实操作闭环，例如导入/启动 Core/代理组可见/选择节点/日志清屏/设置确认中的对应路径。用户截图中出现“按键无反应、字符残留、列不齐、长日志淹没页面”时，按 P0 可用性问题处理，而不是视觉小修。
- 阶段验收不要每次只测一个功能后立刻单点修；非平凡 TUI/CLI/打包改动应批量跑完整验收矩阵，先收集所有问题再按优先级成批修复。矩阵至少覆盖本地 `cargo fmt/check/test`、CLI smoke、TUI TestBackend、Buildx/package smoke、真实 Linux TTY 下 Dashboard/Profiles/Proxies/Logs/Settings/Rules/Connections/Jobs、订阅导入/status/jobs、代理组/节点选择、设置/TUN/system proxy 边界、连接详情/关闭、日志过滤/清屏和任务详情/重试/取消。单点 smoke 只能作为开发中快速反馈，不能替代阶段验收。
- TUI “桌面版可用性对齐”验收要按真实用户路径判断：短命令进入、帮助/快捷键可发现、数字切页、方向键移动、Enter 确认、`/` 过滤、Esc 退出输入/弹窗、`q` 退出都应在真实 PTY 中消费；只证明页面能画出来不算可交付。
- 若 SSH/expect 中 `2/q` 等普通按键未被 TUI 稳定消费，可以把“中文首页/帮助页可见、bracketed paste 模式已启用”记录为界面 smoke 证据，并用 Ctrl-C 或外部 pkill 清理会话；这不等价于完整交互验收，后续仍需修复自动化退出稳定性。
- Buildx 打包网络治理优先使用显式源与缓存：`CLASH_TUI_PACKAGE_DEBIAN_MIRROR`、`CLASH_TUI_PACKAGE_DEBIAN_SECURITY_MIRROR`、`CLASH_TUI_PACKAGE_RUSTUP_DIST_SERVER`、`CLASH_TUI_PACKAGE_CARGO_CACHE_DIR`、`CLASH_TUI_PACKAGE_DOWNLOAD_CACHE_DIR`、`CLASH_TUI_PACKAGE_MIHOMO_*`、`CLASH_TUI_PACKAGE_GEO_*`；不要通过覆盖 `RUSTUP_TOOLCHAIN` 或跳过声明组件来规避 `rust-toolchain.toml`。
- 打包网络经验：不要配置不可信或表现异常的 Docker registry mirror；此前镜像加速站会导致真实下载速度长期为 0。优先使用官方完整 `rust:<toolchain>-bookworm` builder，不要为了省体积默认改成 slim；slim 镜像容易缺少真实打包需要的系统组件，反而把问题推到后面。Debian/Rust 走显式镜像源，mihomo/geo/Cargo 走缓存复用，并在日志中观察实际下载速度。

## 注意事项

- 硬规则：严禁在本地环境启动常驻服务；不要执行 `pnpm web:dev`、`pnpm web:serve`、`pnpm headless:dev`、`cargo run` 或其他会常驻监听端口/启动后端、前端、预览服务的命令。
- 本地 CLI smoke 禁止用 `cargo run` 代替；需要运行本地二进制时先 `cargo build -p clash-tui`，再执行 `target/debug/clash-tui ...`，或直接使用已安装的成品短命令。
- 当前源码、配置和验证结果优先；本文件是项目约定摘要，发现冲突时按当前事实更新。
- 不主动提交、推送、重置或回滚用户已有改动。
- 验证优先使用 cargo check/test、CLI help/smoke、TUI TestBackend、容器 smoke；容器不可用时记录原因和替代验证。
