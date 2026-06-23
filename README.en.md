# Clash TUI

English | [中文](README.md)

Local TUI/CLI controller for mihomo. This branch does not provide a browser UI, HTTP management API, WebSocket API, or static asset server.

## Install

Online install downloads the release archive, verifies it, extracts it to a temporary directory, then delegates to the archive's own `install.sh`:

```bash
BASE_URL=https://example.com/clash-tui/releases/latest/download
curl -fsSL "$BASE_URL/install.sh" | bash -s -- --base-url "$BASE_URL"
```

Pass installer options after `--`:

```bash
curl -fsSL "$BASE_URL/install.sh" | bash -s -- \
  --base-url "$BASE_URL" -- --prefix /opt/clash-tui --no-start
```

Offline install uses the `install.sh` inside the archive:

```bash
tar -xzf clash-tui-linux-x86_64.tar.gz
cd clash-tui-linux-x86_64
sudo ./install.sh
```

Running the installer again updates an existing installation. If the systemd service was active before the update, the installer stops it before replacing files and starts it again afterwards; if it was inactive, the update keeps it inactive. Pass `--no-start` to keep the service stopped in all cases.

For manual verification before offline install:

```bash
sha256sum -c clash-tui-linux-x86_64.tar.gz.sha256
cargo xtask verify-package \
  --archive clash-tui-linux-x86_64.tar.gz \
  --manifest clash-tui-linux-x86_64.manifest.json \
  --bootstrap install.sh
```

## Usage

Start the TUI:

```bash
clash-tui tui
```

No subcommand also enters the TUI:

```bash
clash-tui
```

Common CLI commands:

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

Machine-readable output:

```bash
clash-tui --json core status
```

For private subscription URLs, prefer stdin so the URL is not stored in shell history:

```bash
read -rsp "Subscription URL: " SUB_URL; printf '\n'
printf '%s\n' "$SUB_URL" | clash-tui --json profile import-url --stdin --start-core
unset SUB_URL
```

`profile import-url --stdin` only saves the remote profile. Add `--activate` to switch to it and refresh an already-running core, or `--start-core` on first use to switch to it, generate runtime config, and start the core when it is stopped. Activated imports are transactional: if activation fails, the newly imported profile is rolled back instead of being left as a misleading current profile. After import, verify `proxy groups` returns non-empty groups before treating the subscription as usable.

`profile import-url` returns a redacted import summary instead of echoing the subscription URL. Error messages redact `http://` and `https://` values as `[redacted-url]`.

## Optional Smoke Scripts

Some checks can affect local networking or desktop proxy settings. Run the read-only modes first, and only run confirmed smoke tests in disposable test sessions.

```bash
python3 scripts/clash-tui-tun-linux-smoke.py --preflight --bin clash-tui
scripts/clash-tui-system-proxy-gnome-acceptance.sh --bin clash-tui --output-dir /tmp/clash-tui-gnome-acceptance
```

Confirmed TUN smoke requires `CLASH_TUI_TUN_SMOKE=1`. Confirmed GNOME system-proxy smoke requires passing `--yes` to the acceptance script.

## Configuration

Environment variables:

```text
CLASH_TUI_HOME
CLASH_TUI_RESOURCE_DIR
CLASH_TUI_MIHOMO_BIN
CLASH_TUI_SUBSCRIPTION_CHECK_INTERVAL_SECS
```

The program keeps mihomo controller access internal. Unix platforms use the local IPC controller path generated in the runtime config; non-Unix platforms fall back to loopback-only controller access where supported.

## Development

Allowed non-interactive checks:

```bash
cargo check -p clash-tui
cargo test -p clash-tui
cargo fmt --all --check
cargo xtask ci
```

Do not use `cargo run` or start persistent development services during local verification unless explicitly requested.

Project task commands use the Rust `xtask` entrypoint. No `package.json`, `npm install`, Node runtime, or frontend dependency install is required.

Project process documents:

- [Contributing guide](CONTRIBUTING.md)
- [Security policy](SECURITY.md)

## Packaging

```bash
cargo xtask package --no-docker
```

The package contains the CLI/TUI binary, mihomo binary, resources, and an optional systemd oneshot unit for core lifecycle management. It does not contain browser assets or HTTP/API examples.

Package version metadata intentionally has only one app version source: `[workspace.metadata.clash-tui].app-version` in `Cargo.toml`. The bundled core version is recorded separately as `mihomo.version` in the package manifest.

Installed packages expose a PATH command, usually `/usr/local/bin/clash-tui`, as a symlink to the packaged binary. The binary reads its install layout and finds the packaged `resources/mihomo` automatically, so normal TUI/CLI usage does not require `--home-dir`, `--resource-dir`, or `--mihomo-bin`.

Verify a built archive before uploading or installing it:

```bash
cargo xtask verify-package \
  --archive target/clash-tui-dist/clash-tui-linux-x86_64.tar.gz \
  --manifest target/clash-tui-dist/clash-tui-linux-x86_64.manifest.json
```

For release builds, package from a clean working tree and verify the archive:

```bash
cargo xtask package --target x86_64-unknown-linux-gnu
cargo xtask verify-package \
  --archive target/clash-tui-dist/clash-tui-linux-x86_64.tar.gz \
  --manifest target/clash-tui-dist/clash-tui-linux-x86_64.manifest.json \
  --require-clean-source
```

The GitHub Actions `CD` workflow builds the package on tags or manual dispatch, verifies it, and uploads the online `install.sh`, archive, sidecar manifest, and `.sha256` file.

The tar-internal `manifest.json` intentionally does not rely on the final archive SHA; the archive is sealed before that SHA can be known. Use the sidecar manifest and `.tar.gz.sha256` file as the authoritative archive metadata, and use the tar-internal manifest for package contents, binary/resource hashes, install layout checks, executable tool permissions, and GNOME/TUN smoke entrypoint markers.
