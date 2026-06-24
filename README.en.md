# Clash TUI

English | [中文](README.md)

[![CI](https://github.com/haokejie/clash-tui/actions/workflows/ci.yml/badge.svg)](https://github.com/haokejie/clash-tui/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/haokejie/clash-tui?label=release)](https://github.com/haokejie/clash-tui/releases)
[![License](https://img.shields.io/github/license/haokejie/clash-tui)](LICENSE)

Clash TUI is a local terminal controller for [mihomo](https://github.com/MetaCubeX/mihomo). It puts profile management, subscriptions, proxy groups, node selection, runtime status, logs, connections, rules, TUN, and system proxy controls into a Chinese-first TUI while keeping scriptable CLI commands and JSON output.

This project focuses on local terminal workflows. It does not provide a browser UI, desktop window, public HTTP management API, WebSocket API, or static asset server. The internal controller channel uses local IPC by default; mihomo's external controller is only enabled when the user explicitly turns it on, and it should bind to a local address.

```text
clash-tui
  1 Dashboard    Core, traffic, memory, mode, and quick actions
  2 Proxies      Proxy groups, nodes, latency, and selection state
  3 Profiles     Local profiles, remote subscriptions, current profile
  4 Logs         mihomo logs, filters, and clear action
  5 Settings     IPv6, Allow LAN, DNS, ports, TUN, system proxy
  6 Rules        Rules list and search
  7 Connections  Active connections and close actions
  8 Jobs         Subscription jobs, retry, cancel, and history details
```

## Features

- Run `clash-tui` with no subcommand to open the TUI, suitable for SSH sessions, servers, and headless Linux hosts.
- Manage core lifecycle, profiles, subscriptions, proxy groups, settings, runtime config, rules, connections, providers, jobs, diagnostics, TUN, and system proxy from the CLI.
- Use `--json` for automation-friendly output.
- Import private subscription URLs through stdin; success and error messages redact URLs to avoid leaking them into shell history or logs.
- Release packages include `clash-tui`, mihomo, geo resources, an optional systemd unit, the package installer, and TUN/GNOME system proxy smoke tools.
- The online installer downloads release archives, verifies `.sha256` and sidecar manifest metadata, then delegates to the package-local `install.sh`.

## Support

Prebuilt packages currently target Linux:

| Platform | Package | Status |
| --- | --- | --- |
| Linux x86_64 | `clash-tui-linux-x86_64.tar.gz` | Supported |
| Linux aarch64 | `clash-tui-linux-aarch64.tar.gz` | Supported |
| macOS / Windows | Build from source if needed | TUN, system proxy, and package installation are not release targets yet |

## Install

Install from the latest published release:

```bash
curl -fsSL https://github.com/haokejie/clash-tui/releases/latest/download/install.sh | bash
```

The installer detects the current Linux architecture, downloads the matching archive, and uses `sudo` when it needs to write to `/opt`, `/etc`, `/var/lib`, `/usr/local/bin`, or systemd unit paths.

Select an architecture:

```bash
curl -fsSL https://github.com/haokejie/clash-tui/releases/latest/download/install.sh | \
  bash -s -- --target aarch64
```

Pass package installer options after a second `--`:

```bash
curl -fsSL https://github.com/haokejie/clash-tui/releases/latest/download/install.sh | \
  bash -s -- -- --prefix /opt/clash-tui --no-start
```

Use a mirror or fixed release URL:

```bash
BASE_URL=https://github.com/haokejie/clash-tui/releases/latest/download
curl -fsSL "$BASE_URL/install.sh" | bash -s -- --base-url "$BASE_URL"
```

`latest/download` only works for published GitHub Releases. If a release is still a draft, publish it first, or replace `BASE_URL` with a published `releases/download/<tag>` URL.

## Offline Install

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

Default installed paths:

| Item | Default |
| --- | --- |
| App directory | `/opt/clash-tui` |
| Config directory | `/etc/clash-tui` |
| App data | `/var/lib/clash-tui` |
| PATH command | `/usr/local/bin/clash-tui` |
| systemd unit | `/etc/systemd/system/clash-tui.service` |

Running the installer again updates an existing installation. If the systemd service was active before the update, the installer stops it before replacing files and starts it again afterwards. If it was inactive, the update keeps it inactive. Pass `--no-start` to keep the service stopped.

## First Run

Open the TUI:

```bash
clash-tui
```

Import a remote subscription and start the core:

```bash
read -rsp "Subscription URL: " SUB_URL; printf '\n'
printf '%s\n' "$SUB_URL" | clash-tui --json profile import-url --stdin --start-core
unset SUB_URL
```

Confirm proxy groups are available:

```bash
clash-tui proxy groups
```

Select a node:

```bash
clash-tui proxy select <group> <proxy>
```

Check core status:

```bash
clash-tui core status
```

## Common CLI

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

Machine-readable output:

```bash
clash-tui --json core status
clash-tui --json proxy groups
clash-tui --json diagnose
```

## Configuration

Common environment variables:

| Variable | Description |
| --- | --- |
| `CLASH_TUI_HOME` | Application home directory |
| `CLASH_TUI_RESOURCE_DIR` | mihomo and geo resource directory |
| `CLASH_TUI_MIHOMO_BIN` | mihomo executable path |
| `CLASH_TUI_SUBSCRIPTION_CHECK_INTERVAL_SECS` | Subscription due-check interval, currently used only on TUI startup or manual `--due` updates |

The installer writes a non-secret `install-layout.env` so `clash-tui` can discover its install directory, home, resources, and mihomo binary. Normal usage does not require `--home-dir`, `--resource-dir`, or `--mihomo-bin`.

## Security Boundaries

- Do not put subscription URLs, tokens, SSH keys, raw logs, or production credentials in issues, pull requests, screenshots, or diagnostic reports.
- `profile import-url --stdin` does not echo the subscription URL. Errors redact `http://` and `https://` values as `[redacted-url]`.
- The project does not expose its own HTTP/WS management interface by default. mihomo's external controller is disabled by default and should only bind to local addresses.
- TUN and system proxy operations affect local networking. Run `tun doctor` / `system-proxy doctor` first and keep recovery commands ready before confirmed smoke tests or real toggles.

## Development

Rust 1.91.0 is required. Project automation uses the Rust `xtask` entrypoint; no `package.json`, `npm install`, Node runtime, or frontend dependency install is required.

```bash
cargo fmt --all --check
cargo check -p clash-tui
cargo test -p clash-tui
cargo xtask ci
```

Build a package:

```bash
cargo xtask package --target x86_64-unknown-linux-gnu
```

Verify a package:

```bash
cargo xtask verify-package \
  --archive target/clash-tui-dist/clash-tui-linux-x86_64.tar.gz \
  --manifest target/clash-tui-dist/clash-tui-linux-x86_64.manifest.json
```

Release packages should be built from a clean working tree and verified with `--require-clean-source`. The product version lives only in `[workspace.metadata.clash-tui].app-version` in the root `Cargo.toml`; the bundled mihomo version is recorded separately in `mihomo.version` in the package manifest.

More project documents:

- [Contributing guide](CONTRIBUTING.md)
- [Security policy](SECURITY.md)
- [Package deployment notes](packaging/clash-tui/README.md)
- [Changelog](Changelog.md)

## License

GPL-3.0-only. See [LICENSE](LICENSE).
