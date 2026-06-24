#!/usr/bin/env bash
set -euo pipefail

HOST=""
USER_NAME=""
SSH_PORT="22"
BIN_PATH="clash-tui"
ROWS="40"
COLS="140"
REMOTE_DIR=""
OUTPUT_PATH=""
PASSWORD_ENV="CLASH_TUI_SSH_PASSWORD"
CONNECT_TIMEOUT="15"
KEEP_REMOTE=1

usage() {
  cat <<'EOF'
usage: clash-tui-remote-smoke.sh --host HOST --user USER [options]

Run a real SSH TTY smoke test for the installed clash-tui TUI.
The remote host must already have the target package installed; this script
does not build or install on the server.

Options:
  --target USER@HOST       remote target, alternative to --host/--user
  --host HOST              remote host or IP
  --user USER              remote SSH user
  --port PORT              SSH port, default 22
  --bin COMMAND            remote TUI command, default clash-tui
  --rows ROWS              TTY rows, default 40
  --cols COLS              TTY cols, default 140
  --remote-dir DIR         remote evidence directory under the SSH user
  --output PATH            JSON report path, default target/clash-tui-acceptance/remote-smoke/*/report.json
  --password-env NAME      env var containing SSH password, default CLASH_TUI_SSH_PASSWORD
  --cleanup-remote         remove the remote wrapper/evidence directory after the run
  -h, --help               show this help

Environment:
  CLASH_TUI_SSH_PASSWORD         optional SSH password; key auth is used when empty

The script uploads a small remote wrapper, runs SSH with a real TTY, sets
TERM=xterm-256color and stty rows/cols, enables
CLASH_TUI_TUI_INPUT_TRACE, sends a minimal key sequence, then
checks both the cleaned PTY output and the input trace.
EOF
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 2
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "$1 is required"
}

shell_quote() {
  printf '%q' "$1"
}

timestamp() {
  date +%Y%m%d%H%M%S
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --target)
      target="${2:?missing value for --target}"
      USER_NAME="${target%@*}"
      HOST="${target#*@}"
      [[ "$USER_NAME" != "$HOST" ]] || die "--target must be USER@HOST"
      shift 2
      ;;
    --host)
      HOST="${2:?missing value for --host}"
      shift 2
      ;;
    --user)
      USER_NAME="${2:?missing value for --user}"
      shift 2
      ;;
    --port)
      SSH_PORT="${2:?missing value for --port}"
      shift 2
      ;;
    --bin)
      BIN_PATH="${2:?missing value for --bin}"
      shift 2
      ;;
    --rows)
      ROWS="${2:?missing value for --rows}"
      shift 2
      ;;
    --cols)
      COLS="${2:?missing value for --cols}"
      shift 2
      ;;
    --remote-dir)
      REMOTE_DIR="${2:?missing value for --remote-dir}"
      shift 2
      ;;
    --output)
      OUTPUT_PATH="${2:?missing value for --output}"
      shift 2
      ;;
    --password-env)
      PASSWORD_ENV="${2:?missing value for --password-env}"
      shift 2
      ;;
    --cleanup-remote)
      KEEP_REMOTE=0
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

[[ -n "$HOST" ]] || die "--host is required"
[[ -n "$USER_NAME" ]] || die "--user is required"
[[ "$ROWS" =~ ^[0-9]+$ ]] || die "--rows must be an integer"
[[ "$COLS" =~ ^[0-9]+$ ]] || die "--cols must be an integer"

need_cmd expect
need_cmd python3
need_cmd scp
need_cmd ssh

RUN_ID="$(timestamp)-$$"
REMOTE_TARGET="${USER_NAME}@${HOST}"
if [[ -z "$REMOTE_DIR" ]]; then
  REMOTE_DIR="/tmp/clash-tui-remote-smoke-$RUN_ID"
fi
if [[ -z "$OUTPUT_PATH" ]]; then
  OUTPUT_PATH="target/clash-tui-acceptance/remote-smoke/$RUN_ID/report.json"
fi

PASSWORD="${!PASSWORD_ENV:-}"
TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/clash-tui-remote-smoke.XXXXXX")"
cleanup() {
  rm -rf "$TMP_ROOT"
}
trap cleanup EXIT

OUTPUT_DIR="$(dirname "$OUTPUT_PATH")"
mkdir -p "$OUTPUT_DIR"
RAW_LOG="${OUTPUT_PATH%.json}.raw.log"
CLEAN_LOG="${OUTPUT_PATH%.json}.clean.log"
REMOTE_WRAPPER_LOCAL="$TMP_ROOT/run-tui-smoke.sh"
REMOTE_WRAPPER_PATH="$REMOTE_DIR/run-tui-smoke.sh"
EXPECT_HELPER="$TMP_ROOT/run-command.exp"
EXPECT_TUI="$TMP_ROOT/run-tui.exp"

SSH_COMMON=(-p "$SSH_PORT" -o "StrictHostKeyChecking=accept-new" -o "ConnectTimeout=$CONNECT_TIMEOUT")
SCP_COMMON=(-P "$SSH_PORT" -o "StrictHostKeyChecking=accept-new" -o "ConnectTimeout=$CONNECT_TIMEOUT")

cat > "$REMOTE_WRAPPER_LOCAL" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

BIN_PATH="${1:-clash-tui}"
ROWS="${2:-40}"
COLS="${3:-140}"
WORK_DIR="${4:-/tmp/clash-tui-remote-smoke}"

mkdir -p "$WORK_DIR"
TRACE_FILE="$WORK_DIR/input-trace.log"
rm -f "$TRACE_FILE"

export TERM=xterm-256color
export CLASH_TUI_TUI_INPUT_TRACE="$TRACE_FILE"

if command -v stty >/dev/null 2>&1; then
  stty rows "$ROWS" cols "$COLS" || true
fi

printf 'REMOTE_TUI_SMOKE_BEGIN\n'
set +e
"$BIN_PATH"
rc=$?
set -e
printf '\nTUI_EXITED code=%s\n' "$rc"
printf 'TRACE_BEGIN\n'
if [[ -f "$TRACE_FILE" ]]; then
  sed -n '1,200p' "$TRACE_FILE"
else
  printf 'TRACE_MISSING\n'
fi
printf 'TRACE_END\n'
exit "$rc"
EOF
chmod 755 "$REMOTE_WRAPPER_LOCAL"

cat > "$EXPECT_HELPER" <<'EOF'
set password [lindex $argv 0]
set cmd [lrange $argv 1 end]
set timeout 60
log_user 1
spawn {*}$cmd
expect {
  -re "(?i)are you sure you want to continue connecting" {
    send -- "yes\r"
    exp_continue
  }
  -re {(?i)(password|passphrase).*:|密码.*[:：]} {
    log_user 0
    send -- "$password\r"
    log_user 1
    exp_continue
  }
  eof {}
  timeout {
    exit 124
  }
}
catch wait result
exit [lindex $result 3]
EOF

cat > "$EXPECT_TUI" <<'EOF'
set password [lindex $argv 0]
set cmd [lrange $argv 1 end]
set timeout 45
set sent_keys 0
log_user 1
spawn {*}$cmd
expect {
  -re "(?i)are you sure you want to continue connecting" {
    send -- "yes\r"
    exp_continue
  }
  -re {(?i)(password|passphrase).*:|密码.*[:：]} {
    log_user 0
    send -- "$password\r"
    log_user 1
    exp_continue
  }
  -re "clash-tui|运行概览|总览" {
    if {$sent_keys == 0} {
      after 4500
      send -- "2"
      after 500
      send -- "1"
      after 500
      send -- "g"
      after 500
      send -- "\033"
      after 500
      send -- "q"
      set sent_keys 1
    }
    exp_continue
  }
  eof {}
  timeout {
    if {$sent_keys == 0} {
      send -- "\003"
    } else {
      send -- "\033"
      after 250
      send -- "q"
      after 500
      send -- "\003"
    }
    exit 124
  }
}
catch wait result
exit [lindex $result 3]
EOF

run_with_optional_password() {
  if [[ -n "$PASSWORD" ]]; then
    expect "$EXPECT_HELPER" "$PASSWORD" "$@"
  else
    "$@"
  fi
}

printf 'Remote target: %s\n' "$REMOTE_TARGET"
printf 'Remote TUI command: %s\n' "$BIN_PATH"
printf 'Remote evidence dir: %s\n' "$REMOTE_DIR"

run_with_optional_password ssh "${SSH_COMMON[@]}" "$REMOTE_TARGET" \
  "mkdir -p -- $(shell_quote "$REMOTE_DIR") && chmod 700 -- $(shell_quote "$REMOTE_DIR")"
run_with_optional_password scp "${SCP_COMMON[@]}" "$REMOTE_WRAPPER_LOCAL" \
  "$REMOTE_TARGET:$REMOTE_WRAPPER_PATH"
run_with_optional_password ssh "${SSH_COMMON[@]}" "$REMOTE_TARGET" \
  "chmod 700 -- $(shell_quote "$REMOTE_WRAPPER_PATH")"

REMOTE_COMMAND="exec $(shell_quote "$REMOTE_WRAPPER_PATH") $(shell_quote "$BIN_PATH") $(shell_quote "$ROWS") $(shell_quote "$COLS") $(shell_quote "$REMOTE_DIR")"
set +e
if [[ -n "$PASSWORD" ]]; then
  expect "$EXPECT_TUI" "$PASSWORD" \
    ssh -tt "${SSH_COMMON[@]}" "$REMOTE_TARGET" "$REMOTE_COMMAND" > "$RAW_LOG" 2>&1
  EXPECT_RC=$?
else
  expect "$EXPECT_TUI" "" \
    ssh -tt "${SSH_COMMON[@]}" "$REMOTE_TARGET" "$REMOTE_COMMAND" > "$RAW_LOG" 2>&1
  EXPECT_RC=$?
fi
set -e

python3 - "$RAW_LOG" "$CLEAN_LOG" "$OUTPUT_PATH" "$REMOTE_TARGET" "$BIN_PATH" "$REMOTE_DIR" "$ROWS" "$COLS" "$EXPECT_RC" <<'PY'
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path

raw_path = Path(sys.argv[1])
clean_path = Path(sys.argv[2])
output_path = Path(sys.argv[3])
remote_target = sys.argv[4]
bin_path = sys.argv[5]
remote_dir = sys.argv[6]
rows = int(sys.argv[7])
cols = int(sys.argv[8])
expect_rc = int(sys.argv[9])

raw = raw_path.read_text(errors="replace")
ansi = re.compile(
    r"\x1b(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1b\\))"
)
clean = ansi.sub("", raw).replace("\r", "\n")
clean_path.write_text(clean)

markers = {
    "title": "clash-tui" in clean or "clash-tui" in raw,
    "dashboard": "总览" in clean or "总览" in raw,
    "overview": "运行概览" in clean or "运行概览" in raw,
    "proxy_selector": "代理选择" in clean or "代理选择" in raw,
    "quick_switches": "快速开关" in clean or "快速开关" in raw,
    "mode_switch": "模式切换" in clean or "模式切换" in raw,
    "group_popup": "选择代理组" in clean or "选择代理组" in raw,
    "help_popup": "键位" in clean or "帮助" in clean or "键位" in raw or "帮助" in raw,
    "footer_keys": "键位：" in clean or "键位：" in raw,
    "footer_more_indicator": "…更多" in clean or "…更多" in raw,
    "tui_exited": "TUI_EXITED code=0" in clean or "TUI_EXITED code=0" in raw,
    "trace_begin": "TRACE_BEGIN" in clean or "TRACE_BEGIN" in raw,
    "trace_end": "TRACE_END" in clean or "TRACE_END" in raw,
}
trace = {
    "char": len(re.findall(r"key code=char", clean)),
    "esc": len(re.findall(r"key code=esc", clean)),
    "total": len(re.findall(r"key code=", clean)),
}
required = [
    "title",
    "dashboard",
    "overview",
    "proxy_selector",
    "quick_switches",
    "mode_switch",
    "footer_keys",
    "tui_exited",
    "trace_begin",
    "trace_end",
]
missing = [name for name in required if not markers[name]]
if trace["char"] < 4:
    missing.append("input-trace-char>=4")
if trace["esc"] < 1:
    missing.append("input-trace-esc>=1")
if expect_rc != 0:
    missing.append(f"expect-rc={expect_rc}")

report = {
    "schemaVersion": 1,
    "ok": not missing,
    "createdAt": datetime.now(timezone.utc).isoformat(),
    "remoteTarget": remote_target,
    "bin": bin_path,
    "remoteDir": remote_dir,
    "tty": {"rows": rows, "cols": cols, "term": "xterm-256color"},
    "keys": ["2", "1", "g", "Esc", "q"],
    "markers": markers,
    "trace": trace,
    "expectRc": expect_rc,
    "missing": missing,
    "evidence": {
        "rawLog": str(raw_path),
        "cleanLog": str(clean_path),
    },
}
output_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
print(json.dumps({
    "ok": report["ok"],
    "missing": missing,
    "trace": trace,
    "report": str(output_path),
    "rawLog": str(raw_path),
    "cleanLog": str(clean_path),
}, ensure_ascii=False))
if missing:
    sys.exit(1)
PY

if [[ "$KEEP_REMOTE" -eq 0 ]]; then
  run_with_optional_password ssh "${SSH_COMMON[@]}" "$REMOTE_TARGET" \
    "rm -rf -- $(shell_quote "$REMOTE_DIR")"
fi

printf 'TUI remote smoke report: %s\n' "$OUTPUT_PATH"
printf 'Raw PTY log: %s\n' "$RAW_LOG"
printf 'Clean PTY log: %s\n' "$CLEAN_LOG"
