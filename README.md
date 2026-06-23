# Clash TUI

[English](README.en.md) | 中文

本项目是 mihomo 的本地 TUI/CLI 控制器。本分支不提供浏览器 UI、HTTP 管理 API、WebSocket API 或静态资源服务。

## 安装

在线安装会下载 release 归档、校验归档、解压到临时目录，然后委托归档内置的 `install.sh` 执行安装：

```bash
BASE_URL=https://example.com/clash-tui/releases/latest/download
curl -fsSL "$BASE_URL/install.sh" | bash -s -- --base-url "$BASE_URL"
```

安装器参数放在 `--` 后面：

```bash
curl -fsSL "$BASE_URL/install.sh" | bash -s -- \
  --base-url "$BASE_URL" -- --prefix /opt/clash-tui --no-start
```

离线安装使用归档内的 `install.sh`：

```bash
tar -xzf clash-tui-linux-x86_64.tar.gz
cd clash-tui-linux-x86_64
sudo ./install.sh
```

重复执行安装器会更新已有安装。如果更新前 systemd service 处于 active，安装器会先停止服务、替换文件、再重新启动；如果更新前服务未运行，更新后仍保持停止。传入 `--no-start` 可始终保持服务停止。

离线安装前可手动校验：

```bash
sha256sum -c clash-tui-linux-x86_64.tar.gz.sha256
cargo xtask verify-package \
  --archive clash-tui-linux-x86_64.tar.gz \
  --manifest clash-tui-linux-x86_64.manifest.json \
  --bootstrap install.sh
```

## 使用

启动 TUI：

```bash
clash-tui tui
```

不传子命令也会进入 TUI：

```bash
clash-tui
```

常用 CLI 命令：

```bash
clash-tui core status
clash-tui core start
clash-tui core stop
clash-tui core restart
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
clash-tui settings set ipv6 on
clash-tui settings set allow-lan off
clash-tui settings set unified-delay on
clash-tui settings set log-level info
clash-tui settings set mixed-port 7897
clash-tui settings set dns off
clash-tui subscription update <id>
clash-tui subscription update --all
clash-tui subscription update --due
clash-tui subscription status
clash-tui tun status
clash-tui tun doctor
clash-tui system-proxy status
```

机器可读输出：

```bash
clash-tui --json core status
```

导入私有订阅 URL 时，建议通过 stdin 传入，避免 URL 留在 shell history：

```bash
read -rsp "Subscription URL: " SUB_URL; printf '\n'
printf '%s\n' "$SUB_URL" | clash-tui --json profile import-url --stdin --start-core
unset SUB_URL
```

`profile import-url --stdin` 只保存远程 Profile。添加 `--activate` 会切换到该 Profile 并刷新已运行的 Core；首次使用建议加 `--start-core`，它会切换 Profile、生成 runtime config，并在 Core 停止时启动 Core。带激活的导入是事务式的：如果激活失败，新导入的 Profile 会回滚，不会留下误导性的当前 Profile。导入后应确认 `proxy groups` 返回非空代理组，再把订阅视为可用。

`profile import-url` 返回脱敏后的导入摘要，不会回显订阅 URL。错误信息会把 `http://` 和 `https://` 值脱敏为 `[redacted-url]`。

## 可选 Smoke 脚本

部分检查可能影响本机网络或桌面代理设置。请先运行只读模式，只在一次性测试会话中运行确认式 smoke。

```bash
python3 scripts/clash-tui-tun-linux-smoke.py --preflight --bin clash-tui
scripts/clash-tui-system-proxy-gnome-acceptance.sh --bin clash-tui --output-dir /tmp/clash-tui-gnome-acceptance
```

确认式 TUN smoke 需要设置 `CLASH_TUI_TUN_SMOKE=1`。确认式 GNOME system-proxy smoke 需要给 acceptance 脚本传入 `--yes`。

## 配置

环境变量：

```text
CLASH_TUI_HOME
CLASH_TUI_RESOURCE_DIR
CLASH_TUI_MIHOMO_BIN
CLASH_TUI_SUBSCRIPTION_CHECK_INTERVAL_SECS
```

程序会把 mihomo controller 访问保持在内部。Unix 平台使用 runtime config 中生成的本地 IPC controller path；非 Unix 平台在支持时回退到仅 loopback 的 controller 访问。

## 开发

允许的非交互检查：

```bash
cargo check -p clash-tui
cargo test -p clash-tui
cargo fmt --all --check
cargo xtask ci
```

除非明确要求，不要在本地验证时使用 `cargo run` 或启动常驻开发服务。

项目任务统一使用 Rust `xtask` 入口。不需要 `package.json`、`npm install`、Node runtime 或前端依赖安装。

项目流程文档：

- [贡献指南](CONTRIBUTING.md)
- [安全策略](SECURITY.md)

## 打包

```bash
cargo xtask package --no-docker
```

包内包含 CLI/TUI 二进制、mihomo 二进制、资源文件，以及用于 Core 生命周期管理的可选 systemd oneshot unit。包内不包含浏览器资源或 HTTP/API 示例。

包版本元数据只有一个 app 版本来源：`Cargo.toml` 中的 `[workspace.metadata.clash-tui].app-version`。内置 Core 版本会单独记录在包 manifest 的 `mihomo.version` 中。

安装后的包会暴露 PATH 命令，通常是 `/usr/local/bin/clash-tui`，它是指向包内二进制的符号链接。二进制会读取安装布局并自动找到包内的 `resources/mihomo`，正常 TUI/CLI 使用不需要传 `--home-dir`、`--resource-dir` 或 `--mihomo-bin`。

上传或安装前校验已构建归档：

```bash
cargo xtask verify-package \
  --archive target/clash-tui-dist/clash-tui-linux-x86_64.tar.gz \
  --manifest target/clash-tui-dist/clash-tui-linux-x86_64.manifest.json
```

release 构建应从干净工作树打包并校验归档：

```bash
cargo xtask package --target x86_64-unknown-linux-gnu
cargo xtask verify-package \
  --archive target/clash-tui-dist/clash-tui-linux-x86_64.tar.gz \
  --manifest target/clash-tui-dist/clash-tui-linux-x86_64.manifest.json \
  --require-clean-source
```

GitHub Actions `CD` workflow 会在推送 `v*` tag 或手动触发时构建包、校验包，并上传在线 `install.sh`、归档、sidecar manifest 和 `.sha256` 文件。

tar 内部的 `manifest.json` 不依赖最终归档 SHA；归档封存后才能知道该 SHA。归档元数据以 sidecar manifest 和 `.tar.gz.sha256` 为准；tar 内部 manifest 用于校验包内容、二进制/资源哈希、安装布局、工具可执行权限，以及 GNOME/TUN smoke 入口标记。
