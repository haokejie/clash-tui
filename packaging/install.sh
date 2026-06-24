#!/usr/bin/env bash
set -euo pipefail

DEFAULT_BASE_URL="https://github.com/haokejie/clash-tui/releases/latest/download"
BASE_URL="${CLASH_TUI_INSTALL_BASE_URL:-$DEFAULT_BASE_URL}"
TARGET=""
NO_SUDO="${CLASH_TUI_INSTALL_NO_SUDO:-0}"
KEEP_TEMP=0
INSTALL_ARGS=()

usage() {
  cat <<'EOF'
usage: install.sh [options] [-- installer-options...]

Online bootstrap installer for clash-tui. It downloads a release archive,
verifies it, extracts it, then delegates all real installation work to the
archive's own install.sh.

Options:
  --base-url URL    Override the release asset base URL containing install.sh,
                   tar.gz, manifest, and sha256 files. Defaults to:
                   https://github.com/haokejie/clash-tui/releases/latest/download
                   Can also be set with CLASH_TUI_INSTALL_BASE_URL.
  --target TARGET   Package target or arch. Supported: x86_64, aarch64,
                   x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu.
                   Defaults to the current Linux machine arch.
  --no-sudo         Call the package install.sh directly instead of using sudo.
                   Intended for root shells, containers, and tests.
  --keep-temp       Keep the temporary download/extract directory after exit.
  -h, --help        Show this help.

Examples:
  curl -fsSL https://github.com/haokejie/clash-tui/releases/latest/download/install.sh | bash

  curl -fsSL https://github.com/haokejie/clash-tui/releases/latest/download/install.sh | bash -s -- \
    -- --prefix /opt/clash-tui --no-start
EOF
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 2
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "$1 is required"
}

strip_trailing_slash() {
  printf '%s\n' "${1%/}"
}

while (($#)); do
  case "$1" in
    --base-url)
      BASE_URL="${2:?--base-url requires a value}"
      shift 2
      ;;
    --target)
      TARGET="${2:?--target requires a value}"
      shift 2
      ;;
    --no-sudo)
      NO_SUDO=1
      shift
      ;;
    --keep-temp)
      KEEP_TEMP=1
      shift
      ;;
    --)
      shift
      INSTALL_ARGS=("$@")
      break
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown option before --: $1"
      ;;
  esac
done

package_arch_from_target() {
  case "$1" in
    x86_64|amd64|x86_64-unknown-linux-gnu)
      printf 'x86_64\n'
      ;;
    aarch64|arm64|aarch64-unknown-linux-gnu)
      printf 'aarch64\n'
      ;;
    *)
      return 1
      ;;
  esac
}

detect_package_arch() {
  if [[ -n "$TARGET" ]]; then
    package_arch_from_target "$TARGET" || die "unsupported target: $TARGET"
    return
  fi

  local os_name
  local machine
  os_name="${CLASH_TUI_INSTALL_UNAME_S:-$(uname -s)}"
  machine="${CLASH_TUI_INSTALL_UNAME_M:-$(uname -m)}"
  if [[ "$os_name" != "Linux" ]]; then
    die "online install currently supports Linux only; got $os_name"
  fi
  package_arch_from_target "$machine" || die "unsupported machine arch: $machine"
}

download_file() {
  local url="$1"
  local output="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fL --retry 3 --connect-timeout 20 -o "$output" "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget -O "$output" "$url"
  else
    die "curl or wget is required"
  fi
}

sha256_file() {
  local file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file" | awk '{print $1}'
  else
    die "sha256sum or shasum is required"
  fi
}

expected_sha_from_file() {
  local file="$1"
  awk 'NF >= 1 {print $1; exit}' "$file"
}

archive_sha_from_manifest() {
  local file="$1"
  sed -n '/"archive"[[:space:]]*:/,/[}]/ {
    s/.*"sha256"[[:space:]]*:[[:space:]]*"\([0-9a-fA-F]\{64\}\)".*/\1/p
  }' "$file" | head -n 1
}

verify_archive() {
  local archive="$1"
  local sha_file="$2"
  local manifest_file="$3"
  local expected
  local actual
  local manifest_sha
  local expected_lc
  local actual_lc
  local manifest_sha_lc

  expected="$(expected_sha_from_file "$sha_file")"
  [[ "$expected" =~ ^[0-9a-fA-F]{64}$ ]] || die "invalid sha256 file: $sha_file"
  actual="$(sha256_file "$archive")"
  expected_lc="$(printf '%s' "$expected" | tr '[:upper:]' '[:lower:]')"
  actual_lc="$(printf '%s' "$actual" | tr '[:upper:]' '[:lower:]')"
  if [[ "$actual_lc" != "$expected_lc" ]]; then
    die "archive sha256 mismatch: expected $expected, got $actual"
  fi

  manifest_sha="$(archive_sha_from_manifest "$manifest_file" || true)"
  manifest_sha_lc="$(printf '%s' "$manifest_sha" | tr '[:upper:]' '[:lower:]')"
  if [[ -n "$manifest_sha" && "$manifest_sha_lc" != "$expected_lc" ]]; then
    die "manifest archive sha256 mismatch: expected $expected, got $manifest_sha"
  fi
}

BASE_URL="$(strip_trailing_slash "$BASE_URL")"
[[ -n "$BASE_URL" ]] || die "release base URL is empty"

need_cmd tar
PACKAGE_ARCH="$(detect_package_arch)"
PACKAGE_NAME="clash-tui-linux-${PACKAGE_ARCH}"
ARCHIVE_NAME="${PACKAGE_NAME}.tar.gz"
MANIFEST_NAME="${PACKAGE_NAME}.manifest.json"
SHA_NAME="${ARCHIVE_NAME}.sha256"

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/clash-tui-install.XXXXXX")"
cleanup() {
  if [[ "$KEEP_TEMP" != "1" ]]; then
    rm -rf "$TMP_DIR"
  else
    printf 'kept temporary directory: %s\n' "$TMP_DIR"
  fi
}
trap cleanup EXIT

printf 'clash-tui online install\n'
printf 'target package: %s\n' "$PACKAGE_NAME"
printf 'download base: %s\n' "$BASE_URL"

download_file "$BASE_URL/$ARCHIVE_NAME" "$TMP_DIR/$ARCHIVE_NAME"
download_file "$BASE_URL/$SHA_NAME" "$TMP_DIR/$SHA_NAME"
download_file "$BASE_URL/$MANIFEST_NAME" "$TMP_DIR/$MANIFEST_NAME"
verify_archive "$TMP_DIR/$ARCHIVE_NAME" "$TMP_DIR/$SHA_NAME" "$TMP_DIR/$MANIFEST_NAME"

EXTRACT_DIR="$TMP_DIR/extract"
mkdir -p "$EXTRACT_DIR"
tar -xzf "$TMP_DIR/$ARCHIVE_NAME" -C "$EXTRACT_DIR"
PACKAGE_DIR="$EXTRACT_DIR/$PACKAGE_NAME"
[[ -x "$PACKAGE_DIR/install.sh" ]] || die "package install.sh is missing or not executable"

printf 'archive verified: %s\n' "$ARCHIVE_NAME"
printf 'delegating to package installer: %s/install.sh\n' "$PACKAGE_NAME"

if [[ "$(id -u)" == "0" || "$NO_SUDO" == "1" ]]; then
  bash "$PACKAGE_DIR/install.sh" "${INSTALL_ARGS[@]}"
else
  need_cmd sudo
  sudo bash "$PACKAGE_DIR/install.sh" "${INSTALL_ARGS[@]}"
fi
