#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SMOKE_SCRIPT="$SCRIPT_DIR/clash-tui-system-proxy-gnome-smoke.py"
BIN_PATH="${CLASH_TUI_BIN:-clash-tui}"
OUTPUT_DIR="${CLASH_TUI_GNOME_ACCEPTANCE_DIR:-/tmp/clash-tui-gnome-acceptance}"
ARCHIVE_PATH="${CLASH_TUI_GNOME_ACCEPTANCE_ARCHIVE:-}"
CONFIRMED=0
ALLOW_ROOT=0

usage() {
  cat <<'EOF'
usage: clash-tui-system-proxy-gnome-acceptance.sh [options]

Run the GNOME system proxy acceptance workflow:
  1. read-only preflight
  2. confirmed on/off smoke, only with --yes
  3. read-only verify-report

Options:
  --bin PATH           clash-tui binary or short command
  --output-dir DIR     directory for preflight/smoke/verified JSON reports
  --archive PATH       optional tar.gz evidence archive after full verification
  --smoke-script PATH  path to clash-tui-system-proxy-gnome-smoke.py
  --yes                allow the confirmed system-proxy on/off smoke
  --allow-root         pass --allow-root to the smoke script
  -h, --help           show this help

Environment:
  CLASH_TUI_BIN
  CLASH_TUI_GNOME_ACCEPTANCE_DIR
  CLASH_TUI_GNOME_ACCEPTANCE_ARCHIVE

Without --yes, this script stops after a successful preflight and prints the
next command. It never runs system-proxy on/off unless --yes is provided.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --bin)
      BIN_PATH="${2:?missing value for --bin}"
      shift 2
      ;;
    --output-dir)
      OUTPUT_DIR="${2:?missing value for --output-dir}"
      shift 2
      ;;
    --archive)
      ARCHIVE_PATH="${2:?missing value for --archive}"
      shift 2
      ;;
    --smoke-script)
      SMOKE_SCRIPT="${2:?missing value for --smoke-script}"
      shift 2
      ;;
    --yes)
      CONFIRMED=1
      shift
      ;;
    --allow-root)
      ALLOW_ROOT=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'unknown option: %s\n\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ ! -f "$SMOKE_SCRIPT" ]]; then
  printf 'smoke script not found: %s\n' "$SMOKE_SCRIPT" >&2
  exit 2
fi

mkdir -p "$OUTPUT_DIR"
PREFLIGHT_REPORT="$OUTPUT_DIR/gnome-preflight.json"
SMOKE_REPORT="$OUTPUT_DIR/gnome-smoke.json"
VERIFIED_REPORT="$OUTPUT_DIR/gnome-smoke-verified.json"
ACCEPTANCE_REPORT="$OUTPUT_DIR/gnome-acceptance-verified.json"
SHA_REPORT="$OUTPUT_DIR/gnome-acceptance-SHA256SUMS.txt"

write_sha_report() {
  if ! command -v sha256sum >/dev/null 2>&1; then
    printf 'sha256sum is required to create acceptance evidence hashes\n' >&2
    return 2
  fi

  (
    cd "$OUTPUT_DIR"
    sha256sum \
      gnome-preflight.json \
      gnome-smoke.json \
      gnome-smoke-verified.json \
      gnome-acceptance-verified.json \
      > "$(basename "$SHA_REPORT")"
  )
}

write_archive() {
  local archive_path="$1"
  local archive_dir
  local archive_base
  local archive_abs

  if [[ -z "$archive_path" ]]; then
    return 0
  fi
  if ! command -v tar >/dev/null 2>&1; then
    printf 'tar is required to create an evidence archive\n' >&2
    return 2
  fi

  archive_dir="$(dirname "$archive_path")"
  archive_base="$(basename "$archive_path")"
  mkdir -p "$archive_dir"
  archive_abs="$(cd "$archive_dir" && pwd)/$archive_base"

  (
    cd "$OUTPUT_DIR"
    tar -czf "$archive_abs" \
      gnome-preflight.json \
      gnome-smoke.json \
      gnome-smoke-verified.json \
      gnome-acceptance-verified.json \
      "$(basename "$SHA_REPORT")"
  )

  printf 'Evidence archive: %s\n' "$archive_abs"
  printf 'Evidence archive SHA256: %s\n' "$(sha256sum "$archive_abs" | awk '{print $1}')"
}

ALLOW_ROOT_ARG=()
if [[ "$ALLOW_ROOT" -eq 1 ]]; then
  ALLOW_ROOT_ARG=(--allow-root)
fi

printf 'GNOME system proxy acceptance output: %s\n' "$OUTPUT_DIR"
printf 'Running read-only preflight...\n'
if ! python3 "$SMOKE_SCRIPT" \
  --preflight \
  ${ALLOW_ROOT_ARG[@]+"${ALLOW_ROOT_ARG[@]}"} \
  --bin "$BIN_PATH" \
  --output "$PREFLIGHT_REPORT"; then
  printf 'Preflight failed. Review: %s\n' "$PREFLIGHT_REPORT" >&2
  exit 1
fi

printf 'Preflight passed: %s\n' "$PREFLIGHT_REPORT"
if [[ "$CONFIRMED" -ne 1 ]]; then
  ARCHIVE_HINT=""
  if [[ -n "$ARCHIVE_PATH" ]]; then
    ARCHIVE_HINT=" --archive $(printf '%q' "$ARCHIVE_PATH")"
  fi
  cat >&2 <<EOF
Confirmed smoke was not run.
Review the preflight report, then rerun with:
  $0 --bin $(printf '%q' "$BIN_PATH") --output-dir $(printf '%q' "$OUTPUT_DIR")$ARCHIVE_HINT --yes
EOF
  exit 2
fi

printf 'Running confirmed system-proxy on/off smoke...\n'
CLASH_TUI_SYSTEM_PROXY_SMOKE=1 python3 "$SMOKE_SCRIPT" \
  ${ALLOW_ROOT_ARG[@]+"${ALLOW_ROOT_ARG[@]}"} \
  --yes \
  --bin "$BIN_PATH" \
  --output "$SMOKE_REPORT"

printf 'Verifying confirmed smoke report...\n'
python3 "$SMOKE_SCRIPT" \
  --verify-report "$SMOKE_REPORT" \
  --output "$VERIFIED_REPORT"

printf 'Verifying acceptance report bundle...\n'
python3 "$SMOKE_SCRIPT" \
  --verify-acceptance-dir "$OUTPUT_DIR" \
  --output "$ACCEPTANCE_REPORT"

write_sha_report
write_archive "$ARCHIVE_PATH"

cat <<EOF
GNOME system proxy acceptance completed.
Preflight report: $PREFLIGHT_REPORT
Smoke report: $SMOKE_REPORT
Verified report: $VERIFIED_REPORT
Acceptance report: $ACCEPTANCE_REPORT
SHA256 sums: $SHA_REPORT
EOF
