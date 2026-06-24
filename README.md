# Clash TUI

[English](README.en.md) | 中文

[![CI](https://github.com/haokejie/clash-tui/actions/workflows/ci.yml/badge.svg)](https://github.com/haokejie/clash-tui/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/haokejie/clash-tui?label=release)](https://github.com/haokejie/clash-tui/releases)
[![License](https://img.shields.io/github/license/haokejie/clash-tui)](LICENSE)

Clash TUI 是面向 [mihomo](https://github.com/MetaCubeX/mihomo) 的本地终端控制器。它把常用的 Profile、订阅、代理组、节点选择、运行状态、日志、连接、规则、TUN 和系统代理管理放进一个中文 TUI，同时保留可脚本化的 CLI 和 JSON 输出。

本项目专注本机终端体验，不提供浏览器 UI、桌面窗口、对外 HTTP 管理 API、WebSocket API 或静态资源服务。内部控制通道默认使用本地 IPC；mihomo 外部控制器只有在用户显式开启时才会绑定本机地址。

```text
clash-tui
  1 运行概览    Core、流量、内存、当前模式和快速动作
  2 代理选择    策略组、节点、延迟和选择状态
  3 Profiles   本地配置、远程订阅、当前 Profile 和更新状态
  4 日志        mihomo 日志摘要、过滤和清屏
  5 设置        IPv6、Allow LAN、DNS、端口、TUN、系统代理
  6 规则        规则列表和搜索
  7 连接        活跃连接查看和关闭
  8 任务        订阅更新、重试、取消和历史详情
```

## 功能

- 默认执行 `clash-tui` 进入中文 TUI，适合 SSH、服务器和无桌面 Linux 环境。
- 内置 CLI 子命令，可管理 Core 生命周期、Profile、订阅、代理组、设置、runtime、规则、连接、Provider、任务、诊断、TUN 和系统代理。
- 支持 `--json` 机器可读输出，便于自动化脚本和远端 smoke。
- 远程订阅导入建议走 stdin，成功和错误输出会脱敏 URL，避免订阅地址进入 shell history 或日志。
- 安装包包含 `clash-tui`、mihomo、geo 资源、可选 systemd unit、包内 installer、TUN/GNOME system proxy smoke 工具。
- 在线安装会下载 release 归档、校验 `.sha256` 和 sidecar manifest，再委托包内 `install.sh` 执行真实安装。

## 支持状态

预构建包目前面向 Linux：

| 平台 | 包名 | 状态 |
| --- | --- | --- |
| Linux x86_64 | `clash-tui-linux-x86_64.tar.gz` | 支持 |
| Linux aarch64 | `clash-tui-linux-aarch64.tar.gz` | 支持 |
| macOS / Windows | 源码可尝试构建 | TUN、system proxy、打包安装暂未作为发布目标 |

## 安装

最新正式 release 的安装入口：

```bash
curl -fsSL https://github.com/haokejie/clash-tui/releases/latest/download/install.sh | bash
```

安装器会根据当前 Linux 架构下载对应包，并在需要写入 `/opt`、`/etc`、`/var/lib`、`/usr/local/bin` 或 systemd unit 时自动使用 `sudo`。

指定架构：

```bash
curl -fsSL https://github.com/haokejie/clash-tui/releases/latest/download/install.sh | \
  bash -s -- --target aarch64
```

传递包内 installer 参数时，用第二个 `--` 分隔：

```bash
curl -fsSL https://github.com/haokejie/clash-tui/releases/latest/download/install.sh | \
  bash -s -- -- --prefix /opt/clash-tui --no-start
```

使用镜像或固定 release 地址：

```bash
BASE_URL=https://github.com/haokejie/clash-tui/releases/latest/download
curl -fsSL "$BASE_URL/install.sh" | bash -s -- --base-url "$BASE_URL"
```

`latest/download` 只对已发布的 GitHub Release 可用；如果 release 仍是 Draft，请先发布，或把 `BASE_URL` 换成已发布 tag 的 `releases/download/<tag>` 地址。

## 离线安装

```bash
BASE_URL=https://github.com/haokejie/clash-tui/releases/latest/download
curl -fLO "$BASE_URL/clash-tui-linux-x86_64.tar.gz"
curl -fLO "$BASE_URL/clash-tui-linux-x86_64.tar.gz.sha256"
curl -fLO "$BASE_URL/clash-tui-linux-x86_64.manifest.json"

sha256sum -c clash-tui-linux-x86_64.tar.gz.sha256
tar -xzf clash-tui-linux-x86_64.tar.gz
cd clash-tui-linux-x86_64
sudo ./install.sh
```

安装后默认路径：

| 内容 | 默认路径 |
| --- | --- |
| 程序目录 | `/opt/clash-tui` |
| 配置目录 | `/etc/clash-tui` |
| 应用数据 | `/var/lib/clash-tui` |
| PATH 命令 | `/usr/local/bin/clash-tui` |
| systemd unit | `/etc/systemd/system/clash-tui.service` |

重复执行安装器会更新已有安装。如果更新前 systemd service 为 active，安装器会先停止服务、替换文件、再恢复启动；如果更新前未运行，更新后仍保持停止。传 `--no-start` 可始终保持服务停止。

## 首次使用

打开 TUI：

```bash
clash-tui
```

导入远程订阅并启动 Core：

```bash
read -rsp "Subscription URL: " SUB_URL; printf '\n'
printf '%s\n' "$SUB_URL" | clash-tui --json profile import-url --stdin --start-core
unset SUB_URL
```

确认代理组非空：

```bash
clash-tui proxy groups
```

选择节点：

```bash
clash-tui proxy select <group> <proxy>
```

查看 Core 状态：

```bash
clash-tui core status
```

## 常用 CLI

```bash
clash-tui core start|stop|restart|status|logs
clash-tui mode get
clash-tui mode set rule
clash-tui profile list
clash-tui profile current
clash-tui profile switch <id>
clash-tui profile import-local ./profile.yaml
clash-tui profile import-url --stdin --start-core
clash-tui proxy groups
clash-tui proxy select <group> <proxy>
clash-tui settings show
clash-tui settings set mixed-port 7897
clash-tui settings set dns off
clash-tui subscription update --due
clash-tui subscription status
clash-tui tun doctor
clash-tui system-proxy doctor
clash-tui diagnose
```

机器可读输出：

```bash
clash-tui --json core status
clash-tui --json proxy groups
clash-tui --json diagnose
```

## 配置

常用环境变量：

| 变量 | 说明 |
| --- | --- |
| `CLASH_TUI_HOME` | 应用 home 目录 |
| `CLASH_TUI_RESOURCE_DIR` | mihomo 和 geo 资源目录 |
| `CLASH_TUI_MIHOMO_BIN` | mihomo 可执行文件路径 |
| `CLASH_TUI_SUBSCRIPTION_CHECK_INTERVAL_SECS` | 订阅到期检查间隔，当前仅在 TUI 启动或手动 `--due` 时使用 |

安装包会写入非敏感的 `install-layout.env`，让 `clash-tui` 自动找到安装目录、home、资源目录和 mihomo。正常使用不需要手写 `--home-dir`、`--resource-dir` 或 `--mihomo-bin`。

## 安全边界

- 订阅 URL、token、SSH 密钥、原始日志和生产凭证不应写入 issue、PR、截图或诊断报告。
- `profile import-url --stdin` 不回显订阅 URL，错误信息会把 `http://` 和 `https://` 值脱敏为 `[redacted-url]`。
- 默认不开放本项目自己的 HTTP/WS 管理接口；mihomo 外部控制器默认关闭，并且只应绑定本机地址。
- TUN 和 system proxy 会影响本机网络。执行确认式 smoke 或真实开关前，请先运行 `tun doctor` / `system-proxy doctor` 并准备恢复命令。

## 开发

需要 Rust 1.91.0。项目任务统一通过 Rust `xtask` 入口执行，不需要 `package.json`、`npm install`、Node runtime 或前端依赖。

```bash
cargo fmt --all --check
cargo check -p clash-tui
cargo test -p clash-tui
cargo xtask ci
```

打包：

```bash
cargo xtask package --target x86_64-unknown-linux-gnu
```

校验包：

```bash
cargo xtask verify-package \
  --archive target/clash-tui-dist/clash-tui-linux-x86_64.tar.gz \
  --manifest target/clash-tui-dist/clash-tui-linux-x86_64.manifest.json
```

发布前应从干净工作树打包，并使用 `--require-clean-source` 校验归档。产品版本号只维护在根 `Cargo.toml` 的 `[workspace.metadata.clash-tui].app-version`；内置 mihomo 版本记录在包 manifest 的 `mihomo.version`。

更多流程见：

- [贡献指南](CONTRIBUTING.md)
- [安全策略](SECURITY.md)
- [包内部署说明](packaging/clash-tui/README.md)
- [变更日志](Changelog.md)

## License

GPL-3.0-only. See [LICENSE](LICENSE).
