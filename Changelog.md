# Changelog

## v0.2.1 - 2026-06-24

### Changed

- Reworked the public README and English README around open-source first-run, install, support, security, development, and release workflows.
- Made the online installer default to `https://github.com/haokejie/clash-tui/releases/latest/download`, while keeping `--base-url` and `CLASH_TUI_INSTALL_BASE_URL` for mirrors and tests.
- Added repository, homepage, readme, description, keyword, category, and explicit non-publish metadata to workspace packages.

## v0.2.0 - 2026-06-24

### Added

- First packaged Linux release with `x86_64` and `aarch64` archives.
- Added online bootstrap installer, archive SHA256 verification, sidecar package manifests, and package-local installer delegation.
- Included `clash-tui`, bundled mihomo, geo resources, optional systemd unit, and smoke tools for Linux TUN and GNOME system proxy checks.
