#!/usr/bin/env bash
set -euo pipefail

PREFIX="/opt/clash-tui"
CONFIG_DIR="/etc/clash-tui"
HOME_DIR="/var/lib/clash-tui"
SERVICE_NAME="clash-tui.service"
BIN_DIR="/usr/local/bin"
BIN_NAME="clash-tui"
ARCHIVE=""
ENABLE=1
START=1
BACKUP=1
BIN_LINK=1

usage() {
  cat <<'EOF'
usage: install.sh [options]

Options:
  --prefix <dir>         Default: /opt/clash-tui
  --config-dir <dir>     Default: /etc/clash-tui
  --home-dir <dir>       Default: /var/lib/clash-tui
  --service-name <name>  Default: clash-tui.service
  --bin-dir <dir>        Default: /usr/local/bin
  --bin-name <name>      Default: clash-tui
  --no-bin-link          Do not install a PATH symlink.
  --no-enable            Do not enable the systemd unit.
  --no-start             Do not start the systemd unit.
  --no-backup            Do not keep a timestamped backup of an existing prefix.
  -h, --help             Show this help.
EOF
}

while (($#)); do
  case "$1" in
    --archive)
      ARCHIVE="${2:?--archive requires a value}"
      shift 2
      ;;
    --prefix)
      PREFIX="${2:?--prefix requires a value}"
      shift 2
      ;;
    --config-dir)
      CONFIG_DIR="${2:?--config-dir requires a value}"
      shift 2
      ;;
    --home-dir)
      HOME_DIR="${2:?--home-dir requires a value}"
      shift 2
      ;;
    --service-name)
      SERVICE_NAME="${2:?--service-name requires a value}"
      shift 2
      ;;
    --bin-dir)
      BIN_DIR="${2:?--bin-dir requires a value}"
      shift 2
      ;;
    --bin-name)
      BIN_NAME="${2:?--bin-name requires a value}"
      shift 2
      ;;
    --no-bin-link)
      BIN_LINK=0
      shift
      ;;
    --no-enable)
      ENABLE=0
      shift
      ;;
    --no-start)
      START=0
      shift
      ;;
    --no-backup)
      BACKUP=0
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ "$(id -u)" != "0" ]]; then
  echo "install.sh must run as root" >&2
  exit 1
fi
if [[ "$BIN_NAME" == */* ]]; then
  echo "--bin-name must be a command name, not a path" >&2
  exit 2
fi

make_abs() {
  case "$1" in
    /*) printf '%s\n' "$1" ;;
    *) printf '%s/%s\n' "$(pwd)" "$1" ;;
  esac
}

sed_replacement() {
  printf '%s' "$1" | sed -e 's/[#&]/\\&/g'
}

restore_selinux_context() {
  if command -v restorecon >/dev/null 2>&1; then
    restorecon -R "$@"
  fi
}

require_safe_prefix() {
  case "$1" in
    /|/bin|/boot|/dev|/etc|/home|/lib|/lib64|/opt|/proc|/root|/run|/sbin|/sys|/tmp|/usr|/usr/bin|/usr/local|/var|/var/lib)
      echo "--prefix must be an application directory, got: $1" >&2
      exit 2
      ;;
  esac
}

manifest_package_name() {
  local file="$1"
  if [[ -f "$file" ]]; then
    sed -n 's/.*"packageName"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$file" | head -n 1
  fi
}

systemd_available() {
  command -v systemctl >/dev/null 2>&1 && [[ -d /run/systemd/system ]]
}

service_state() {
  if [[ "${SYSTEMD_AVAILABLE:-0}" != "1" ]]; then
    printf 'unavailable\n'
    return
  fi
  local state
  state="$(systemctl is-active "$SERVICE_NAME" 2>/dev/null || true)"
  if [[ -n "$state" ]]; then
    printf '%s\n' "$state"
  else
    printf 'unknown\n'
  fi
}

PREFIX="$(make_abs "$PREFIX")"
CONFIG_DIR="$(make_abs "$CONFIG_DIR")"
HOME_DIR="$(make_abs "$HOME_DIR")"
BIN_DIR="$(make_abs "$BIN_DIR")"
SERVICE_PATH="/etc/systemd/system/$SERVICE_NAME"
require_safe_prefix "$PREFIX"
SYSTEMD_AVAILABLE=0
if systemd_available; then
  SYSTEMD_AVAILABLE=1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR=""
SOURCE_DIR="$SCRIPT_DIR"

cleanup() {
  if [[ -n "$WORK_DIR" ]]; then
    rm -rf "$WORK_DIR"
  fi
}
trap cleanup EXIT

if [[ -n "$ARCHIVE" ]]; then
  WORK_DIR="$(mktemp -d)"
  tar -xzf "$ARCHIVE" -C "$WORK_DIR"
  SOURCE_DIR="$(find "$WORK_DIR" -mindepth 1 -maxdepth 1 -type d | head -n 1)"
fi

if [[ ! -x "$SOURCE_DIR/clash-tui" ]]; then
  echo "clash-tui binary not found in $SOURCE_DIR" >&2
  exit 1
fi

EXISTING_INSTALL=0
if [[ -x "$PREFIX/clash-tui" || -f "$PREFIX/manifest.json" || -f "$PREFIX/install-layout.env" || -f "$SERVICE_PATH" ]]; then
  EXISTING_INSTALL=1
fi

OLD_PACKAGE="$(manifest_package_name "$PREFIX/manifest.json")"
NEW_PACKAGE="$(manifest_package_name "$SOURCE_DIR/manifest.json")"
[[ -n "$OLD_PACKAGE" ]] || OLD_PACKAGE="unknown"
[[ -n "$NEW_PACKAGE" ]] || NEW_PACKAGE="unknown"

SERVICE_PREVIOUS_STATE="$(service_state)"
SERVICE_WAS_ACTIVE=0
if [[ "$SYSTEMD_AVAILABLE" == "1" ]] && systemctl is-active --quiet "$SERVICE_NAME" 2>/dev/null; then
  SERVICE_WAS_ACTIVE=1
fi

SHOULD_START=0
if [[ "$START" == "1" ]]; then
  if [[ "$EXISTING_INSTALL" == "1" ]]; then
    if [[ "$SERVICE_WAS_ACTIVE" == "1" ]]; then
      SHOULD_START=1
    fi
  else
    SHOULD_START=1
  fi
fi

if [[ "$EXISTING_INSTALL" == "1" ]]; then
  echo
  echo "Existing installation detected"
  echo "  install dir:       $PREFIX"
  echo "  old package:       $OLD_PACKAGE"
  echo "  new package:       $NEW_PACKAGE"
  echo "  service:           $SERVICE_NAME"
  echo "  previous state:    $SERVICE_PREVIOUS_STATE"
fi

if [[ "$SERVICE_WAS_ACTIVE" == "1" ]]; then
  echo "Stopping active service before update: $SERVICE_NAME"
  systemctl stop "$SERVICE_NAME"
fi

if [[ -d "$PREFIX" && "$BACKUP" == "1" ]]; then
  cp -a "$PREFIX" "$PREFIX.bak.$(date +%Y%m%d%H%M%S)"
fi

if [[ -d "$PREFIX" ]]; then
  rm -rf "$PREFIX"
fi
install -d -m 755 "$PREFIX"
cp -a "$SOURCE_DIR/." "$PREFIX/"
chown -R root:root "$PREFIX"
find "$PREFIX" -type d -exec chmod 755 {} +
find "$PREFIX" -type f -exec chmod 644 {} +
chmod +x "$PREFIX/clash-tui"
if [[ -f "$PREFIX/resources/mihomo" ]]; then
  chmod +x "$PREFIX/resources/mihomo"
fi
find "$PREFIX/tools" -type f \( -name '*.sh' -o -name '*.py' \) -exec chmod +x {} + 2>/dev/null || true
restore_selinux_context "$PREFIX"

install -d -m 700 "$CONFIG_DIR"
install -d -m 755 "$HOME_DIR"
if [[ ! -f "$CONFIG_DIR/env" && -f "$PREFIX/env.example" ]]; then
  install -m 600 "$PREFIX/env.example" "$CONFIG_DIR/env"
fi

cat > "$PREFIX/install-layout.env" <<EOF
# Generated by clash-tui install.sh. This file contains no secrets.
CLASH_TUI_HOME=$HOME_DIR
CLASH_TUI_RESOURCE_DIR=$PREFIX/resources
CLASH_TUI_MIHOMO_BIN=$PREFIX/resources/mihomo
CLASH_TUI_SERVICE_NAME=$SERVICE_NAME
EOF
chmod 644 "$PREFIX/install-layout.env"

if [[ "$BIN_LINK" == "1" ]]; then
  install -d -m 755 "$BIN_DIR"
  BIN_PATH="$BIN_DIR/$BIN_NAME"
  BIN_TARGET="$PREFIX/clash-tui"
  if [[ -L "$BIN_PATH" ]]; then
    CURRENT_TARGET="$(readlink "$BIN_PATH")"
    if [[ "$CURRENT_TARGET" != "$BIN_TARGET" ]]; then
      echo "$BIN_PATH already points to $CURRENT_TARGET; use --bin-name or remove it first" >&2
      exit 1
    fi
  elif [[ -e "$BIN_PATH" ]]; then
    echo "$BIN_PATH already exists and is not a symlink; use --bin-name or remove it first" >&2
    exit 1
  else
    ln -s "$BIN_TARGET" "$BIN_PATH"
  fi
fi

SERVICE_CURRENT_STATE="unavailable"
if [[ "$SYSTEMD_AVAILABLE" == "1" && -f "$PREFIX/systemd/clash-tui.service" ]]; then
  install -m 644 "$PREFIX/systemd/clash-tui.service" "$SERVICE_PATH"

  PREFIX_SED="$(sed_replacement "$PREFIX")"
  CONFIG_DIR_SED="$(sed_replacement "$CONFIG_DIR")"
  HOME_DIR_SED="$(sed_replacement "$HOME_DIR")"
  SERVICE_NAME_SED="$(sed_replacement "$SERVICE_NAME")"
  sed -i \
    -e "s#WorkingDirectory=/opt/clash-tui#WorkingDirectory=$PREFIX_SED#g" \
    -e "s#CLASH_TUI_HOME=/var/lib/clash-tui#CLASH_TUI_HOME=$HOME_DIR_SED#g" \
    -e "s#CLASH_TUI_RESOURCE_DIR=/opt/clash-tui/resources#CLASH_TUI_RESOURCE_DIR=$PREFIX_SED/resources#g" \
    -e "s#CLASH_TUI_MIHOMO_BIN=/opt/clash-tui/resources/mihomo#CLASH_TUI_MIHOMO_BIN=$PREFIX_SED/resources/mihomo#g" \
    -e "s#CLASH_TUI_SERVICE_NAME=clash-tui.service#CLASH_TUI_SERVICE_NAME=$SERVICE_NAME_SED#g" \
    -e "s#EnvironmentFile=-/etc/clash-tui/env#EnvironmentFile=-$CONFIG_DIR_SED/env#g" \
    -e "s#/opt/clash-tui/clash-tui#$PREFIX_SED/clash-tui#g" \
    "$SERVICE_PATH"
  restore_selinux_context "$SERVICE_PATH"

  systemctl daemon-reload
  if [[ "$ENABLE" == "1" ]]; then
    systemctl enable "$SERVICE_NAME"
  fi
  if [[ "$SHOULD_START" == "1" ]]; then
    systemctl restart "$SERVICE_NAME"
  fi
  SERVICE_CURRENT_STATE="$(service_state)"
else
  echo "systemd not available; skipping service installation"
fi

CLI_COMMAND="$PREFIX/clash-tui"
if [[ "$BIN_LINK" == "1" ]]; then
  CLI_COMMAND="$BIN_DIR/$BIN_NAME"
fi

echo
if [[ "$EXISTING_INSTALL" == "1" ]]; then
  echo "clash-tui updated"
else
  echo "clash-tui installed"
fi
echo "  install dir: $PREFIX"
echo "  config dir:  $CONFIG_DIR"
echo "  data dir:    $HOME_DIR"
echo "  resources:   $PREFIX/resources"
if [[ "$SYSTEMD_AVAILABLE" == "1" ]]; then
  echo "  service:     $SERVICE_NAME"
  echo "  service was: $SERVICE_PREVIOUS_STATE"
  echo "  service now: $SERVICE_CURRENT_STATE"
else
  echo "  service:     unavailable (systemd/systemctl not detected)"
fi
echo "  command:     $CLI_COMMAND"
echo
echo "Use it:"
echo "  open TUI:       $CLI_COMMAND"
echo "  open TUI also:  $CLI_COMMAND tui"
echo "  show help:      $CLI_COMMAND --help"
echo "  core status:    $CLI_COMMAND core status"
if [[ "$SYSTEMD_AVAILABLE" == "1" ]]; then
  echo "  service status: systemctl status $SERVICE_NAME"
else
  echo "  core start:     $CLI_COMMAND core start"
  echo "  core stop:      $CLI_COMMAND core stop"
fi
