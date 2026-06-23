# Contributing

This repository ships `clash-tui`, a local TUI/CLI controller for mihomo. Keep changes small, reviewable, and aligned with the existing Rust and script layout.

## Development Rules

- Use the existing workspace structure under `crates/`, `scripts/`, and `packaging/`.
- Do not reintroduce a browser UI, public HTTP management API, WebSocket API, or static asset server.
- Keep user-facing defaults Chinese unless the surrounding command or protocol requires English.
- Do not commit subscription URLs, tokens, SSH credentials, raw PTY logs, customer data, or local acceptance evidence.
- Prefer stdin for private subscription URLs so they do not enter shell history.

## Required Checks

Run the smallest relevant check while developing, then run the full local check before opening a pull request:

```bash
cargo xtask ci
```

The CI gate covers formatting, workspace build checks, supply-chain metadata checks, clippy, script syntax checks, script tests, and Rust tests.

For packaging or installer changes, also build and verify a package:

```bash
cargo xtask package --target x86_64-unknown-linux-gnu
cargo xtask verify-package \
  --archive target/clash-tui-dist/clash-tui-linux-x86_64.tar.gz \
  --manifest target/clash-tui-dist/clash-tui-linux-x86_64.manifest.json
```

## Pull Requests

Each pull request should include:

- What changed and why.
- The validation commands that passed.
- Any skipped validation and the reason.
- Operational risk or migration notes when behavior changes.
