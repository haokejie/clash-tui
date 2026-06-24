# Clash TUI 终端版部署说明

本包提供本地 `clash-tui` TUI/CLI 控制器和 mihomo 二进制，不包含 Web UI、HTTP API 或 WebSocket 管理面。

## 包结构

```text
clash-tui/
├── clash-tui
├── resources/
│   ├── mihomo
│   ├── Country.mmdb
│   ├── geoip.dat
│   └── geosite.dat
├── tools/
│   ├── clash-tui-system-proxy-gnome-smoke.py
│   ├── clash-tui-system-proxy-gnome-acceptance.sh
│   └── clash-tui-tun-linux-smoke.py
├── systemd/
│   └── clash-tui.service
├── env.example
├── install.sh
├── manifest.json
└── README.md
```

## 安装

离线安装先解压 tar 包，再执行包内唯一安装脚本：

```bash
tar -xzf clash-tui-linux-x86_64.tar.gz
cd clash-tui-linux-x86_64
sudo ./install.sh
```

在线安装由发布目录中的 `install.sh` 负责下载、校验和解压，最终也会调用本包内的 `install.sh`。本包内脚本是唯一真正安装逻辑。

重复执行安装脚本会进入更新模式。脚本会检测已有 `$PREFIX`、manifest、安装布局或同名 systemd unit；更新前会备份旧安装目录，若服务当前为 active，会先停止服务，替换包文件后恢复为 active。若更新前服务没有运行，默认不会因为更新而主动启动；传 `--no-start` 时也不会启动。

常用选项：

```text
--prefix <dir>        默认 /opt/clash-tui
--config-dir <dir>    默认 /etc/clash-tui
--home-dir <dir>      默认 /var/lib/clash-tui
--service-name <name> 默认 clash-tui.service
--bin-dir <dir>       默认 /usr/local/bin
--bin-name <name>     默认 clash-tui
--no-bin-link         不创建 PATH symlink
--no-enable           不设置开机自启
--no-start            只安装，不启动 core
--no-backup           替换旧目录时不保留备份
```

## 使用

安装脚本默认创建短命令：

```text
/usr/local/bin/clash-tui -> /opt/clash-tui/clash-tui
```

二进制会读取安装时生成的 `install-layout.env`，并自动找到对应 home、resources 和包内 mihomo。正常使用不需要手写 `--home-dir`、`--resource-dir` 或 `--mihomo-bin`。

启动 TUI：

```bash
clash-tui tui
```

或直接执行二进制，无子命令时默认进入 TUI：

```bash
clash-tui
```

常用 CLI：

```bash
clash-tui core status
clash-tui core start
clash-tui core stop
clash-tui core run
clash-tui mode get
clash-tui mode set rule
clash-tui profile list
clash-tui profile import-local ./profile.yaml
clash-tui profile import-url --stdin --start-core
clash-tui subscription update --due
clash-tui proxy groups
clash-tui settings show
clash-tui settings set mixed-port 7897
clash-tui settings set dns off
clash-tui tun status
clash-tui tun doctor
clash-tui system-proxy status
```

机器可读输出：

```bash
clash-tui --json core status
```

导入真实订阅时不要把 URL 写进 shell history，建议用 stdin：

```bash
read -rsp "Subscription URL: " SUB_URL; printf '\n'
printf '%s\n' "$SUB_URL" | clash-tui --json profile import-url --stdin --start-core
unset SUB_URL
```

`profile import-url --stdin` 只保存远程 Profile；`--activate` 会切换为当前 Profile、生成 runtime，并在 Core 已运行时刷新 Core；首次使用建议加 `--start-core`，它会在 Core 停止时自动启动。带激活的导入是事务式的：如果激活失败，本次新增 Profile 会回滚，不会留下误导性的 current Profile。导入后应继续执行 `clash-tui proxy groups`，确认返回非空代理组，不能只看导入或订阅更新成功。

`profile import-url` 的成功输出只返回脱敏摘要；错误中的 `http://` 和 `https://` URL 会显示为 `[redacted-url]`。

## Linux TUN 验收

`tun on/off` 会短暂创建 `Meta` TUN 网卡与 `198.18.0.0/30` 路由。先运行只读预检：

```bash
python3 tools/clash-tui-tun-linux-smoke.py \
  --preflight \
  --bin clash-tui \
  --output /tmp/clash-tui-tun-preflight.json
```

确认式 smoke 只应在 Linux 测试会话中运行，并要求已经导入 current Profile、Core 已停止、TUN 已关闭、存在 `/dev/net/tun`，且 root 或 mihomo 已具备 CAP_NET_ADMIN：

```bash
CLASH_TUI_TUN_SMOKE=1 \
  python3 tools/clash-tui-tun-linux-smoke.py \
    --bin clash-tui \
    --output /tmp/clash-tui-tun-smoke.json
```

确认式 smoke 会执行 `tun on`、启动 Core、等待 runtime `tun.enable=true`、`Meta` link 和 `198.18.0.0/30` route 出现，随后执行 `tun off` 与 `core stop`。报告内的 `nextSteps`、`mutated`、`urlLeak` 和 cleanup 信息可作为归档证据。

## GNOME 系统代理验收

`system-proxy on/off` 会尝试修改当前桌面用户的系统代理。无桌面服务器验收可以证明只读预检、安全拒绝和回滚行为，但要证明 GNOME 自动应用真正可用，必须在已登录的 GNOME 桌面用户会话中跑通本流程，不建议用 root 会话代替。

安装包提供了安全恢复脚本，会先保存 GNOME `gsettings` 代理值，再执行 `system-proxy on/off`，最后在 `finally` 中恢复原始设置：

```bash
tools/clash-tui-system-proxy-gnome-acceptance.sh \
  --bin clash-tui \
  --output-dir /tmp/clash-tui-gnome-acceptance
```

预检通过并确认允许短暂改变桌面代理后，再带 `--yes` 运行；脚本会自动执行确认式 smoke，并立刻校验 smoke 报告：

```bash
tools/clash-tui-system-proxy-gnome-acceptance.sh \
  --bin clash-tui \
  --output-dir /tmp/clash-tui-gnome-acceptance \
  --archive /tmp/clash-tui-gnome-acceptance.tar.gz \
  --yes
```

也可以拆开执行底层命令：

```bash
python3 tools/clash-tui-system-proxy-gnome-smoke.py \
  --preflight \
  --bin clash-tui \
  --output /tmp/clash-tui-gnome-preflight.json
```

```bash
CLASH_TUI_SYSTEM_PROXY_SMOKE=1 \
  python3 tools/clash-tui-system-proxy-gnome-smoke.py \
    --bin clash-tui \
    --output /tmp/clash-tui-gnome-smoke.json
```

再用只读报告校验模式确认回传 JSON 足以作为最终证据：

```bash
python3 tools/clash-tui-system-proxy-gnome-smoke.py \
  --verify-report /tmp/clash-tui-gnome-smoke.json \
  --output /tmp/clash-tui-gnome-smoke-verified.json
```

外部桌面跑完整 acceptance 后，也可以只读复核整个输出目录：

```bash
python3 tools/clash-tui-system-proxy-gnome-smoke.py \
  --verify-acceptance-dir /tmp/clash-tui-gnome-acceptance \
  --output /tmp/clash-tui-gnome-acceptance/gnome-acceptance-verified.json
```

acceptance 包装脚本默认只跑只读预检，只有传 `--yes` 才执行确认式 smoke。`--preflight` 是只读预检。确认执行的 smoke 会要求 `system-proxy doctor` 返回 `canAutoApply=true`，并验证开启后 GNOME HTTP/HTTPS/SOCKS 主机和端口已写入、关闭后 mode 变为 `none`。只在允许短暂改变桌面代理的测试会话中运行。`--output` 会用原子替换写出同一份最终 JSON 报告，方便归档 preflight 或 smoke 证据。报告内会记录解析后的 binary path、binary SHA256、DBus 与 DISPLAY/WAYLAND/XDG/DESKTOP 标记是否齐全，`nextSteps` 会说明下一步是修复环境、重跑预检，还是执行确认式 smoke。`--verify-report` 不会修改系统代理，会检查回传 smoke JSON 是否证明非 root GNOME 桌面会话、on/off 成功、GNOME 值写入/关闭、恢复成功且无 URL 泄漏。`--verify-acceptance-dir` 也不会修改系统代理，会检查 acceptance 目录中的 preflight、smoke 和 verified 三份 JSON 是否同时通过并指向同一次桌面验收；最终 JSON 会记录三份源报告的 SHA256，方便归档后核对证据未被替换。完整复核通过后，包装脚本会写出 `gnome-acceptance-SHA256SUMS.txt`；带 `--archive <tar.gz>` 时，还会生成证据归档包，内容只包含四份 JSON 报告和这份 SHA 清单，便于从桌面测试机回传后做只读复核。

## systemd

随包 unit 是本地 Core 生命周期托管，不提供 Web 服务：

```bash
sudo systemctl enable --now clash-tui.service
sudo systemctl status clash-tui.service
```

该 unit 使用 `core run` 前台运行 manager，由 systemd 真正持有 `clash-tui` 与 mihomo 进程。TUI 仍应在用户终端中手动启动；当 Core 由 systemd 管理时，TUI/CLI 的重启操作会委托给 systemd。安装脚本会先检测 `systemctl` 与 systemd 运行环境；不可用时跳过 unit，只安装二进制和资源，可用 `clash-tui core start|status|stop` 手动管理 detached Core。

## 配置

默认路径：

```text
CLASH_TUI_HOME=/var/lib/clash-tui
CLASH_TUI_RESOURCE_DIR=/opt/clash-tui/resources
CLASH_TUI_MIHOMO_BIN=/opt/clash-tui/resources/mihomo
CLASH_TUI_SUBSCRIPTION_CHECK_INTERVAL_SECS=300
```

安装脚本会在安装目录写入非敏感的 `install-layout.env`，用于交互 CLI/TUI 的默认路径解析；systemd 仍读取 `/etc/clash-tui/env` 作为可选覆盖。

需要覆盖 systemd 配置时：

```bash
sudo install -d -m 700 /etc/clash-tui
sudo install -m 600 /opt/clash-tui/env.example /etc/clash-tui/env
sudo ${EDITOR:-vi} /etc/clash-tui/env
sudo systemctl restart clash-tui.service
```

## 验证

```bash
clash-tui --json core status
clash-tui --json profile list
clash-tui --json subscription status
```

上传或安装前建议先在源码仓库构建机校验 tar 包、sidecar manifest、包内 manifest、二进制/资源 SHA 和工具可执行权限：

版本口径只保留两类：`manifest.json` 的 `versions.app` 来自源码根 `Cargo.toml` 的 `app-version`，包内核心版本记录在 `mihomo.version`；Cargo crate 不再作为独立发布版本展示。

```bash
cargo xtask verify-package \
  --archive target/clash-tui-dist/clash-tui-linux-x86_64.tar.gz \
  --manifest target/clash-tui-dist/clash-tui-linux-x86_64.manifest.json
```

tar 包内的 `manifest.json` 不要求携带最终 archive SHA：tar 归档完成后才能计算自身 SHA，因此 archive SHA 以 sidecar manifest 和 `.tar.gz.sha256` 为准；tar 内 manifest 用于校验包内容、二进制/资源 hash、安装布局、工具可执行权限，以及 GNOME/TUN smoke 入口的关键参数标记。

容器或 CI 中建议运行：

```bash
cargo check -p clash-tui
cargo test -p clash-tui
```

## 打包网络参数

本地 Docker Buildx 打包默认使用官方 `rust:<toolchain>-bookworm` builder，并在容器内真实编译 Rust、下载 mihomo 与 geo 资源。网络较慢时优先配置下载源和缓存，不要绕过项目 `rust-toolchain.toml`。

常用环境变量：

```bash
CLASH_TUI_PACKAGE_DEBIAN_MIRROR=https://mirrors.aliyun.com/debian
CLASH_TUI_PACKAGE_DEBIAN_SECURITY_MIRROR=https://mirrors.aliyun.com/debian-security
CLASH_TUI_PACKAGE_RUSTUP_DIST_SERVER=https://mirrors.ustc.edu.cn/rust-static
CLASH_TUI_PACKAGE_CARGO_CACHE_DIR=target/clash-tui-cargo-cache
CLASH_TUI_PACKAGE_DOWNLOAD_CACHE_DIR=target/clash-tui-download-cache
CLASH_TUI_PACKAGE_MIHOMO_VERSION=v1.19.27
CLASH_TUI_PACKAGE_MIHOMO_BASE_URL=https://github.com/MetaCubeX/mihomo/releases/download
CLASH_TUI_PACKAGE_GEO_BASE_URL=https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest
```

也可以单独覆盖资源 URL：

```bash
CLASH_TUI_PACKAGE_GEO_COUNTRY_URL=<country.mmdb URL>
CLASH_TUI_PACKAGE_GEO_GEOSITE_URL=<geosite.dat URL>
CLASH_TUI_PACKAGE_GEO_GEOIP_URL=<geoip.dat URL>
```
