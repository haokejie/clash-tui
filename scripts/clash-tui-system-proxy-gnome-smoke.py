#!/usr/bin/env python3
"""Smoke-test real GNOME system proxy application for clash-tui.

This script intentionally changes the current desktop user's GNOME proxy
settings by running `clash-tui system-proxy on/off`. It snapshots the
GNOME values first and restores them in a finally block.
"""

from __future__ import annotations

import argparse
import ast
import getpass
import hashlib
import json
import os
import shlex
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any


CONFIRM_ENV = "CLASH_TUI_SYSTEM_PROXY_SMOKE"
DEFAULT_BIN_ENV = "CLASH_TUI_BIN"
REPORT_SCHEMA_VERSION = 2

GSETTINGS_KEYS = (
    ("org.gnome.system.proxy", "mode"),
    ("org.gnome.system.proxy", "ignore-hosts"),
    ("org.gnome.system.proxy.http", "host"),
    ("org.gnome.system.proxy.http", "port"),
    ("org.gnome.system.proxy.https", "host"),
    ("org.gnome.system.proxy.https", "port"),
    ("org.gnome.system.proxy.socks", "host"),
    ("org.gnome.system.proxy.socks", "port"),
)

DESKTOP_MARKER_ENV_KEYS = (
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XDG_CURRENT_DESKTOP",
    "DESKTOP_SESSION",
)


class SmokeError(RuntimeError):
    pass


def run(argv: list[str]) -> dict[str, Any]:
    try:
        completed = subprocess.run(argv, text=True, capture_output=True, check=False)
    except OSError as err:
        return {
            "argv": argv,
            "code": 127,
            "stdout": "",
            "stderr": str(err),
        }
    return {
        "argv": argv,
        "code": completed.returncode,
        "stdout": completed.stdout,
        "stderr": completed.stderr,
    }


def fail(message: str) -> None:
    raise SmokeError(message)


def has_url(value: Any) -> bool:
    text = json.dumps(value, ensure_ascii=False, sort_keys=True) if not isinstance(value, str) else value
    return "http://" in text or "https://" in text


def safe_env_value(value: str) -> str:
    value = value.replace("\r", " ").replace("\n", " ").replace("\t", " ").strip()
    if "http://" in value or "https://" in value:
        return "[redacted-url]"
    return value[:120]


def current_user_name() -> str | None:
    try:
        return getpass.getuser()
    except (KeyError, OSError):
        return os.environ.get("USER") or os.environ.get("LOGNAME")


def dbus_address_kind(value: str | None) -> str | None:
    if not value:
        return None
    if value.startswith("unix:path="):
        return "unix-path"
    if value.startswith("unix:abstract="):
        return "unix-abstract"
    if value.startswith("unix:"):
        return "unix"
    return "other"


def desktop_session_report() -> dict[str, Any]:
    dbus_value = os.environ.get("DBUS_SESSION_BUS_ADDRESS")
    markers = {
        key: safe_env_value(value)
        for key in DESKTOP_MARKER_ENV_KEYS
        if (value := os.environ.get(key))
    }
    dbus_present = bool(dbus_value)
    marker_present = bool(markers)
    return {
        "uid": os.getuid() if hasattr(os, "getuid") else None,
        "euid": os.geteuid() if hasattr(os, "geteuid") else None,
        "user": current_user_name(),
        "dbusSessionBusAddressPresent": dbus_present,
        "dbusSessionBusAddressKind": dbus_address_kind(dbus_value),
        "desktopMarkers": markers,
        "desktopMarkerPresent": marker_present,
        "looksLikeDesktopSession": dbus_present and marker_present,
    }


def write_report_file(path: str, text: str) -> None:
    output_path = os.path.abspath(path)
    output_dir = os.path.dirname(output_path) or "."
    os.makedirs(output_dir, exist_ok=True)
    fd, tmp_path = tempfile.mkstemp(
        prefix=f".{os.path.basename(output_path)}.",
        suffix=".tmp",
        dir=output_dir,
        text=True,
    )
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write(text)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(tmp_path, output_path)
    except OSError:
        try:
            os.unlink(tmp_path)
        except OSError:
            pass
        raise


def finish_report(report: dict[str, Any], output_path: str | None) -> int:
    if output_path:
        report["output"] = {
            "path": output_path,
            "ok": True,
        }
    report["nextSteps"] = next_steps_for_report(report)
    report["urlLeak"] = has_url(report)
    if report["urlLeak"]:
        report["ok"] = False

    text = json.dumps(report, ensure_ascii=False, indent=2, sort_keys=True)
    if output_path:
        try:
            write_report_file(output_path, text + "\n")
        except OSError as err:
            report["output"] = {
                "path": output_path,
                "ok": False,
                "error": str(err),
            }
            report["ok"] = False
            report["nextSteps"] = next_steps_for_report(report)
            report["urlLeak"] = has_url(report)
            if report["urlLeak"]:
                report["ok"] = False
            text = json.dumps(report, ensure_ascii=False, indent=2, sort_keys=True)

    print(text)
    return 0 if report["ok"] else 1


def next_steps_for_report(report: dict[str, Any]) -> list[dict[str, str]]:
    mode = str(report.get("mode", ""))
    bin_path = str(report.get("bin") or os.environ.get(DEFAULT_BIN_ENV, "clash-tui"))
    script_path = sys.argv[0] if sys.argv and sys.argv[0].endswith("clash-tui-system-proxy-gnome-smoke.py") else "scripts/clash-tui-system-proxy-gnome-smoke.py"
    script_cmd = f"python3 {shlex.quote(script_path)}"
    bin_arg = shlex.quote(bin_path)

    if report.get("ok"):
        if mode == "preflight":
            return [
                {
                    "code": "run-confirmed-smoke",
                    "message": "预检已通过；可在允许短暂改变桌面代理的已登录 GNOME 用户会话中执行确认式 smoke。",
                    "command": f"{CONFIRM_ENV}=1 {script_cmd} --bin {bin_arg}",
                }
            ]
        if mode == "smoke":
            return [
                {
                    "code": "archive-report",
                    "message": "确认式 smoke 已通过；保存此 JSON 作为 GNOME 系统代理真实写入和恢复证据。",
                }
            ]
        if mode == "verify-report":
            return [
                {
                    "code": "archive-verified-report",
                    "message": "报告校验已通过；可将原始 smoke JSON 与此校验 JSON 一起归档为最终证据。",
                }
            ]
        if mode == "verify-acceptance":
            return [
                {
                    "code": "archive-acceptance-report",
                    "message": "acceptance 目录校验已通过；可将 preflight/smoke/verified JSON 与此报告一起归档为最终证据。",
                }
            ]

    steps: list[dict[str, str]] = []
    error = str(report.get("error") or "")

    if mode == "verify-report":
        steps.append(
            {
                "code": "rerun-confirmed-smoke",
                "message": "该报告尚不能证明 GNOME 系统代理真实 on/off 成功；请在已登录 GNOME 桌面用户会话中重跑只读预检和确认式 smoke。",
            }
        )
    if mode == "verify-acceptance":
        steps.append(
            {
                "code": "rerun-acceptance",
                "message": "acceptance 目录尚不能证明完整 GNOME system proxy 验收通过；请在已登录 GNOME 桌面用户会话中重跑 acceptance --yes。",
            }
        )

    if mode == "smoke" and not report.get("confirmed"):
        steps.append(
            {
                "code": "confirm-mutation",
                "message": "确认式 smoke 会短暂修改桌面代理；先运行只读预检，通过后再设置确认环境变量或传 --yes。",
                "command": f"{script_cmd} --preflight --bin {bin_arg}",
            }
        )

    if (mode == "preflight" and report.get("rootUser") and not report.get("allowRoot")) or "not root" in error:
        steps.append(
            {
                "code": "use-desktop-user",
                "message": "请用已登录 GNOME 桌面用户运行；root/SSH 会话通常不是要修改的桌面代理会话。",
            }
        )

    if mode == "preflight" and not report.get("binaryAvailable", True):
        steps.append(
            {
                "code": "fix-binary",
                "message": "未找到 clash-tui；请安装成品包、加入 PATH，或用 --bin 指向短命令/二进制。",
            }
        )

    schema = report.get("gsettings") if isinstance(report.get("gsettings"), dict) else {}
    if schema and not schema.get("schemaAvailable"):
        steps.append(
            {
                "code": "use-gnome-session",
                "message": "当前环境缺少 org.gnome.system.proxy schema；请在带 GNOME 桌面会话和 gsettings schema 的机器上重跑。",
            }
        )

    desktop_session = report.get("desktopSession") if isinstance(report.get("desktopSession"), dict) else {}
    if mode == "preflight" and desktop_session and not desktop_session.get("looksLikeDesktopSession"):
        steps.append(
            {
                "code": "use-desktop-session",
                "message": "当前环境缺少 DBus 会话或 DISPLAY/WAYLAND/XDG/DESKTOP 标记；请在已登录 GNOME 桌面用户会话中重跑。",
            }
        )

    if "canAutoApply" in json.dumps(report.get("doctor", {}), ensure_ascii=False) and not bool(
        (report.get("doctor") or {}).get("data", {}).get("canAutoApply")
    ):
        steps.append(
            {
                "code": "fix-doctor-checks",
                "message": "system-proxy doctor 尚未允许自动应用；请按 doctor.checks/manualAction 修复环境后重跑只读预检。",
                "command": f"{bin_arg} --json system-proxy doctor",
            }
        )

    output = report.get("output") if isinstance(report.get("output"), dict) else {}
    if output and not output.get("ok", True):
        steps.append(
            {
                "code": "fix-output-path",
                "message": "JSON 报告写入失败；请确认 --output 目标目录存在且当前用户有写入权限，或改用可写路径。",
            }
        )

    restore = report.get("restore") if isinstance(report.get("restore"), dict) else {}
    gsettings_restore = restore.get("gsettings") if isinstance(restore.get("gsettings"), dict) else {}
    if mode == "smoke" and report.get("mutated") and gsettings_restore and not gsettings_restore.get("ok", True):
        steps.append(
            {
                "code": "manual-restore-gsettings",
                "message": "脚本已尝试恢复但 gsettings 校验失败；请参考 beforeGsettings 里的原始值手动恢复桌面代理设置。",
            }
        )

    if not steps:
        steps.append(
            {
                "code": "rerun-preflight",
                "message": "请根据 checks/error 修正环境后重跑只读预检；不要直接在未知会话执行确认式 on/off。",
                "command": f"{script_cmd} --preflight --bin {bin_arg}",
            }
        )

    return steps


def binary_exists(bin_path: str) -> bool:
    return shutil.which(bin_path) is not None or os.path.exists(bin_path)


def binary_report(bin_path: str) -> dict[str, Any]:
    resolved = shutil.which(bin_path) or (os.path.abspath(bin_path) if os.path.exists(bin_path) else None)
    report: dict[str, Any] = {
        "input": bin_path,
        "available": resolved is not None,
        "resolvedPath": resolved,
        "sha256": None,
    }
    if resolved is not None:
        report["sha256"] = sha256_file(Path(resolved))
    return report


def is_root_user() -> bool:
    return hasattr(os, "geteuid") and os.geteuid() == 0


def gsettings(args: list[str]) -> dict[str, Any]:
    return run(["gsettings", *args])


def gsettings_get(schema: str, key: str) -> str:
    result = gsettings(["get", schema, key])
    if result["code"] != 0:
        fail(f"gsettings get {schema} {key} failed: {result['stderr'].strip()}")
    return result["stdout"].strip()


def gsettings_set(schema: str, key: str, value: str) -> dict[str, Any]:
    return gsettings(["set", schema, key, value])


def snapshot_gsettings() -> dict[str, dict[str, str]]:
    snapshot: dict[str, dict[str, str]] = {}
    for schema, key in GSETTINGS_KEYS:
        snapshot[f"{schema} {key}"] = {
            "schema": schema,
            "key": key,
            "value": gsettings_get(schema, key),
        }
    return snapshot


def restore_gsettings(snapshot: dict[str, dict[str, str]]) -> dict[str, Any]:
    results = []
    ok = True
    entries = list(snapshot.values())
    entries.sort(key=lambda entry: 1 if entry["schema"] == "org.gnome.system.proxy" and entry["key"] == "mode" else 0)
    for entry in entries:
        result = gsettings_set(entry["schema"], entry["key"], entry["value"])
        results.append(
            {
                "schema": entry["schema"],
                "key": entry["key"],
                "value": entry["value"],
                "code": result["code"],
                "stderr": result["stderr"].strip(),
            }
        )
        ok = ok and result["code"] == 0
    after = snapshot_gsettings()
    matches = after == snapshot
    return {
        "ok": ok and matches,
        "setResults": results,
        "matchesOriginal": matches,
        "after": after,
    }


def parse_variant_string(raw: str) -> str:
    raw = raw.strip()
    try:
        value = ast.literal_eval(raw)
        if isinstance(value, str):
            return value
    except (SyntaxError, ValueError):
        pass
    if len(raw) >= 2 and raw[0] == "'" and raw[-1] == "'":
        return raw[1:-1]
    return raw


def parse_variant_int(raw: str) -> int:
    parts = raw.strip().split()
    if not parts:
        fail("empty integer gsettings value")
    try:
        return int(parts[-1])
    except ValueError as err:
        raise SmokeError(f"invalid integer gsettings value {raw!r}") from err


def parse_variant_string_array(raw: str) -> list[str]:
    try:
        value = ast.literal_eval(raw)
        if isinstance(value, list):
            return [item for item in value if isinstance(item, str)]
    except (SyntaxError, ValueError):
        pass
    return []


def cli_json(bin_path: str, args: list[str]) -> dict[str, Any]:
    result = run([bin_path, "--json", *args])
    parsed: dict[str, Any] | None = None
    if result["stdout"].strip():
        try:
            parsed = json.loads(result["stdout"])
        except json.JSONDecodeError as err:
            fail(f"{' '.join(args)} returned invalid JSON: {err}")
    return {
        "code": result["code"],
        "stderr": result["stderr"].strip(),
        "json": parsed,
        "data": parsed.get("data", {}) if isinstance(parsed, dict) else {},
    }


def require(condition: bool, message: str, checks: list[dict[str, Any]]) -> None:
    checks.append({"ok": condition, "message": message})
    if not condition:
        fail(message)


def require_schema() -> None:
    if shutil.which("gsettings") is None:
        fail("gsettings is not available")
    schemas = gsettings(["list-schemas"])
    if schemas["code"] != 0:
        fail(f"gsettings list-schemas failed: {schemas['stderr'].strip()}")
    if "org.gnome.system.proxy" not in schemas["stdout"].splitlines():
        fail("org.gnome.system.proxy schema is not available")


def gsettings_schema_report() -> dict[str, Any]:
    gsettings_path = shutil.which("gsettings")
    report: dict[str, Any] = {
        "gsettingsPath": gsettings_path,
        "schemaAvailable": False,
        "listSchemasCode": None,
        "message": None,
    }
    if gsettings_path is None:
        report["message"] = "gsettings is not available"
        return report

    schemas = gsettings(["list-schemas"])
    report["listSchemasCode"] = schemas["code"]
    if schemas["code"] != 0:
        report["message"] = schemas["stderr"].strip()
        return report

    report["schemaAvailable"] = "org.gnome.system.proxy" in schemas["stdout"].splitlines()
    report["message"] = (
        "org.gnome.system.proxy schema is available"
        if report["schemaAvailable"]
        else "org.gnome.system.proxy schema is not available"
    )
    return report


def preflight_report(bin_path: str, allow_root: bool) -> dict[str, Any]:
    checks: list[dict[str, Any]] = []
    root_user = is_root_user()
    binary = binary_report(bin_path)
    bin_available = bool(binary["available"])
    schema = gsettings_schema_report()
    desktop_session = desktop_session_report()
    report: dict[str, Any] = {
        "schemaVersion": REPORT_SCHEMA_VERSION,
        "ok": False,
        "mode": "preflight",
        "bin": bin_path,
        "rootUser": root_user,
        "allowRoot": allow_root,
        "binaryAvailable": bin_available,
        "binary": binary,
        "desktopSession": desktop_session,
        "gsettings": schema,
        "checks": checks,
        "mutated": False,
    }

    checks.append({"ok": bin_available, "message": "clash-tui binary is available"})
    checks.append({
        "ok": (not root_user) or allow_root,
        "message": "running as non-root desktop user or --allow-root was provided",
    })
    checks.append({
        "ok": bool(desktop_session["looksLikeDesktopSession"]),
        "message": "DBus session and desktop/display marker are available",
    })
    checks.append({"ok": bool(schema["schemaAvailable"]), "message": "GNOME proxy schema is available"})

    if schema["schemaAvailable"]:
        try:
            report["gsettingsSnapshot"] = snapshot_gsettings()
        except SmokeError as err:
            report["gsettingsSnapshotError"] = str(err)
            checks.append({"ok": False, "message": f"failed to snapshot GNOME proxy settings: {err}"})

    if bin_available:
        status = cli_json(bin_path, ["system-proxy", "status"])
        doctor = cli_json(bin_path, ["system-proxy", "doctor"])
        report["status"] = status
        report["doctor"] = doctor
        checks.append({"ok": status["code"] == 0, "message": "system-proxy status exits with code 0"})
        checks.append({"ok": doctor["code"] == 0, "message": "system-proxy doctor exits with code 0"})
        checks.append({
            "ok": bool(doctor["data"].get("canAutoApply")),
            "message": "system-proxy doctor reports canAutoApply=true",
        })

    report["ok"] = all(check["ok"] for check in checks)
    return report


def validate_after_on(
    snapshot: dict[str, dict[str, str]],
    endpoint: dict[str, Any],
    checks: list[dict[str, Any]],
) -> None:
    expected_host = str(endpoint.get("host", "127.0.0.1"))
    expected_port = int(endpoint.get("port", 7897))
    expected_bypass = [
        item.strip()
        for item in str(endpoint.get("bypass", "")).split(",")
        if item.strip()
    ]

    require(
        parse_variant_string(snapshot["org.gnome.system.proxy mode"]["value"]) == "manual",
        "GNOME proxy mode is manual after system-proxy on",
        checks,
    )
    for schema in (
        "org.gnome.system.proxy.http",
        "org.gnome.system.proxy.https",
        "org.gnome.system.proxy.socks",
    ):
        require(
            parse_variant_string(snapshot[f"{schema} host"]["value"]) == expected_host,
            f"{schema} host matches {expected_host}",
            checks,
        )
        require(
            parse_variant_int(snapshot[f"{schema} port"]["value"]) == expected_port,
            f"{schema} port matches {expected_port}",
            checks,
        )

    ignore_hosts = parse_variant_string_array(snapshot["org.gnome.system.proxy ignore-hosts"]["value"])
    for host in expected_bypass:
        require(host in ignore_hosts, f"ignore-hosts contains {host}", checks)


def validate_after_off(snapshot: dict[str, dict[str, str]], checks: list[dict[str, Any]]) -> None:
    require(
        parse_variant_string(snapshot["org.gnome.system.proxy mode"]["value"]) == "none",
        "GNOME proxy mode is none after system-proxy off",
        checks,
    )


def report_check(checks: list[dict[str, Any]], condition: bool, message: str, **extra: Any) -> bool:
    entry = {"ok": bool(condition), "message": message}
    entry.update(extra)
    checks.append(entry)
    return bool(condition)


def verify_gsettings_values(
    source_report: dict[str, Any],
    checks: list[dict[str, Any]],
) -> None:
    endpoint = (source_report.get("doctor") or {}).get("data", {}).get("endpoint", {})
    after_on = source_report.get("afterOnGsettings")
    after_off = source_report.get("afterOffGsettings")
    before = source_report.get("beforeGsettings")
    restore = source_report.get("restore") if isinstance(source_report.get("restore"), dict) else {}
    gsettings_restore = restore.get("gsettings") if isinstance(restore.get("gsettings"), dict) else {}

    if report_check(checks, isinstance(after_on, dict), "report contains afterOnGsettings"):
        try:
            validate_after_on(after_on, endpoint, checks)
        except (KeyError, SmokeError) as err:
            report_check(checks, False, f"afterOnGsettings does not prove GNOME proxy on: {err}")

    if report_check(checks, isinstance(after_off, dict), "report contains afterOffGsettings"):
        try:
            validate_after_off(after_off, checks)
        except (KeyError, SmokeError) as err:
            report_check(checks, False, f"afterOffGsettings does not prove GNOME proxy off: {err}")

    report_check(checks, isinstance(before, dict) and bool(before), "report contains beforeGsettings snapshot")
    report_check(checks, bool(gsettings_restore.get("ok")), "restore.gsettings.ok is true")
    report_check(checks, bool(gsettings_restore.get("matchesOriginal")), "restore.gsettings matches original")


def verify_smoke_report(source_report: dict[str, Any], source_path: str | None = None) -> dict[str, Any]:
    checks: list[dict[str, Any]] = []
    desktop_session = source_report.get("desktopSession") if isinstance(source_report.get("desktopSession"), dict) else {}
    doctor = source_report.get("doctor") if isinstance(source_report.get("doctor"), dict) else {}
    doctor_data = doctor.get("data") if isinstance(doctor.get("data"), dict) else {}
    on = source_report.get("on") if isinstance(source_report.get("on"), dict) else {}
    on_data = on.get("data") if isinstance(on.get("data"), dict) else {}
    off = source_report.get("off") if isinstance(source_report.get("off"), dict) else {}
    off_data = off.get("data") if isinstance(off.get("data"), dict) else {}
    restore = source_report.get("restore") if isinstance(source_report.get("restore"), dict) else {}
    gsettings_report = source_report.get("gsettings") if isinstance(source_report.get("gsettings"), dict) else {}
    binary = source_report.get("binary") if isinstance(source_report.get("binary"), dict) else {}

    verification = {
        "schemaVersion": REPORT_SCHEMA_VERSION,
        "ok": False,
        "mode": "verify-report",
        "sourcePath": source_path,
        "mutated": False,
        "checks": checks,
        "sourceSummary": {
            "schemaVersion": source_report.get("schemaVersion"),
            "mode": source_report.get("mode"),
            "ok": source_report.get("ok"),
            "confirmed": source_report.get("confirmed"),
            "mutated": source_report.get("mutated"),
            "urlLeak": source_report.get("urlLeak"),
            "rootUser": source_report.get("rootUser"),
            "allowRoot": source_report.get("allowRoot"),
            "binary": binary,
            "desktopSession": desktop_session,
            "gsettings": gsettings_report,
            "doctorCanAutoApply": doctor_data.get("canAutoApply"),
            "on": {
                "code": on.get("code"),
                "enabled": on_data.get("enabled"),
                "configSaved": on_data.get("configSaved"),
                "platformApplied": on_data.get("platformApplied"),
            },
            "off": {
                "code": off.get("code"),
                "configSaved": off_data.get("configSaved"),
                "platformApplied": off_data.get("platformApplied"),
            },
            "restore": {
                "initialAppEnabled": restore.get("initialAppEnabled"),
                "appStateKnown": restore.get("appStateKnown"),
                "appRestoreCode": (restore.get("appRestore") or {}).get("code")
                if isinstance(restore.get("appRestore"), dict)
                else None,
                "gsettingsOk": (restore.get("gsettings") or {}).get("ok")
                if isinstance(restore.get("gsettings"), dict)
                else None,
                "gsettingsMatchesOriginal": (restore.get("gsettings") or {}).get("matchesOriginal")
                if isinstance(restore.get("gsettings"), dict)
                else None,
            },
        },
    }

    report_check(checks, source_report.get("schemaVersion") == REPORT_SCHEMA_VERSION, "source report schemaVersion is current")
    report_check(checks, source_report.get("mode") == "smoke", "source report mode is smoke")
    report_check(checks, source_report.get("ok") is True, "source report ok=true")
    report_check(checks, source_report.get("confirmed") is True, "source report was explicitly confirmed")
    report_check(checks, source_report.get("mutated") is True, "source report mutated=true")
    report_check(checks, source_report.get("urlLeak") is False, "source report has urlLeak=false")
    report_check(checks, source_report.get("rootUser") is False, "source report ran as non-root desktop user")
    report_check(checks, binary.get("available") is True, "source report binary was available")
    report_check(
        checks,
        isinstance(binary.get("sha256"), str) and len(binary.get("sha256")) == 64,
        "source report contains binary SHA256",
    )
    report_check(checks, desktop_session.get("looksLikeDesktopSession") is True, "source report ran in a desktop session")
    report_check(checks, gsettings_report.get("schemaAvailable") is True, "GNOME proxy schema was available")
    report_check(checks, doctor.get("code") == 0, "system-proxy doctor exited 0")
    report_check(checks, doctor_data.get("canAutoApply") is True, "system-proxy doctor reported canAutoApply=true")
    report_check(checks, on.get("code") == 0, "system-proxy on exited 0")
    report_check(checks, on_data.get("enabled") is True, "system-proxy on reported enabled=true")
    report_check(checks, on_data.get("configSaved") is True, "system-proxy on reported configSaved=true")
    report_check(checks, on_data.get("platformApplied") is True, "system-proxy on reported platformApplied=true")
    report_check(checks, off.get("code") == 0, "system-proxy off exited 0")
    report_check(checks, off_data.get("configSaved") is True, "system-proxy off reported configSaved=true")
    report_check(checks, off_data.get("platformApplied") is True, "system-proxy off reported platformApplied=true")
    verify_gsettings_values(source_report, checks)

    verification["ok"] = all(check["ok"] for check in checks)
    return verification


def verify_report_file(path: str) -> dict[str, Any]:
    try:
        source_report = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as err:
        return {
            "schemaVersion": REPORT_SCHEMA_VERSION,
            "ok": False,
            "mode": "verify-report",
            "sourcePath": path,
            "mutated": False,
            "checks": [{"ok": False, "message": f"failed to read report JSON: {err}"}],
        }
    if not isinstance(source_report, dict):
        return {
            "schemaVersion": REPORT_SCHEMA_VERSION,
            "ok": False,
            "mode": "verify-report",
            "sourcePath": path,
            "mutated": False,
            "checks": [{"ok": False, "message": "report JSON root must be an object"}],
        }
    return verify_smoke_report(source_report, path)


def read_report_object(path: Path, label: str, checks: list[dict[str, Any]]) -> dict[str, Any] | None:
    try:
        report = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as err:
        report_check(checks, False, f"{label} report can be read", path=str(path), error=str(err))
        return None
    if not isinstance(report, dict):
        report_check(checks, False, f"{label} report JSON root is an object", path=str(path))
        return None
    report_check(checks, True, f"{label} report can be read", path=str(path))
    return report


def sha256_file(path: Path) -> str | None:
    try:
        digest = hashlib.sha256()
        with path.open("rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(chunk)
        return digest.hexdigest()
    except OSError:
        return None


def report_field(report: dict[str, Any] | None, *keys: str) -> Any:
    value: Any = report
    for key in keys:
        if not isinstance(value, dict):
            return None
        value = value.get(key)
    return value


def report_summary(report: dict[str, Any] | None) -> dict[str, Any]:
    if not isinstance(report, dict):
        return {}
    return {
        "schemaVersion": report.get("schemaVersion"),
        "mode": report.get("mode"),
        "ok": report.get("ok"),
        "confirmed": report.get("confirmed"),
        "mutated": report.get("mutated"),
        "urlLeak": report.get("urlLeak"),
        "bin": report.get("bin"),
        "binary": report.get("binary"),
        "rootUser": report.get("rootUser"),
        "desktopSessionLooksLike": report_field(report, "desktopSession", "looksLikeDesktopSession"),
        "desktopSessionUser": report_field(report, "desktopSession", "user"),
        "gsettingsSchemaAvailable": report_field(report, "gsettings", "schemaAvailable"),
        "doctorCanAutoApply": report_field(report, "doctor", "data", "canAutoApply"),
    }


def verify_acceptance_dir(dir_path: str) -> dict[str, Any]:
    checks: list[dict[str, Any]] = []
    source_dir = Path(dir_path)
    preflight_path = source_dir / "gnome-preflight.json"
    smoke_path = source_dir / "gnome-smoke.json"
    verified_path = source_dir / "gnome-smoke-verified.json"

    verification: dict[str, Any] = {
        "schemaVersion": REPORT_SCHEMA_VERSION,
        "ok": False,
        "mode": "verify-acceptance",
        "sourceDir": str(source_dir),
        "mutated": False,
        "checks": checks,
        "reports": {
            "preflight": str(preflight_path),
            "smoke": str(smoke_path),
            "verified": str(verified_path),
        },
        "reportHashes": {
            "preflight": {
                "path": str(preflight_path),
                "sha256": sha256_file(preflight_path),
            },
            "smoke": {
                "path": str(smoke_path),
                "sha256": sha256_file(smoke_path),
            },
            "verified": {
                "path": str(verified_path),
                "sha256": sha256_file(verified_path),
            },
        },
    }

    if not report_check(checks, source_dir.is_dir(), "acceptance directory exists", path=str(source_dir)):
        return verification

    preflight = read_report_object(preflight_path, "preflight", checks)
    smoke = read_report_object(smoke_path, "smoke", checks)
    verified = read_report_object(verified_path, "verified", checks)

    verification["reportSummaries"] = {
        "preflight": report_summary(preflight),
        "smoke": report_summary(smoke),
        "verified": report_summary(verified),
    }

    if preflight is not None:
        report_check(checks, preflight.get("schemaVersion") == REPORT_SCHEMA_VERSION, "preflight schemaVersion is current")
        report_check(checks, preflight.get("mode") == "preflight", "preflight report mode is preflight")
        report_check(checks, preflight.get("ok") is True, "preflight report ok=true")
        report_check(checks, preflight.get("mutated") is False, "preflight report mutated=false")
        report_check(checks, preflight.get("urlLeak") is False, "preflight report has urlLeak=false")
        report_check(checks, preflight.get("rootUser") is False, "preflight ran as non-root desktop user")
        report_check(checks, report_field(preflight, "binary", "available") is True, "preflight binary was available")
        report_check(
            checks,
            isinstance(report_field(preflight, "binary", "sha256"), str)
            and len(report_field(preflight, "binary", "sha256")) == 64,
            "preflight contains binary SHA256",
        )
        report_check(
            checks,
            report_field(preflight, "desktopSession", "looksLikeDesktopSession") is True,
            "preflight ran in a desktop session",
        )
        report_check(
            checks,
            report_field(preflight, "gsettings", "schemaAvailable") is True,
            "preflight found GNOME proxy schema",
        )
        report_check(
            checks,
            report_field(preflight, "doctor", "data", "canAutoApply") is True,
            "preflight doctor reported canAutoApply=true",
        )

    smoke_verification: dict[str, Any] | None = None
    if smoke is not None:
        smoke_verification = verify_smoke_report(smoke, str(smoke_path))
        verification["smokeVerification"] = smoke_verification
        report_check(checks, smoke_verification["ok"], "smoke report passes verify-report checks")

    if verified is not None:
        verified_summary = verified.get("sourceSummary") if isinstance(verified.get("sourceSummary"), dict) else {}
        report_check(checks, verified.get("schemaVersion") == REPORT_SCHEMA_VERSION, "verified schemaVersion is current")
        report_check(checks, verified.get("mode") == "verify-report", "verified report mode is verify-report")
        report_check(checks, verified.get("ok") is True, "verified report ok=true")
        report_check(checks, verified.get("mutated") is False, "verified report mutated=false")
        report_check(checks, verified.get("urlLeak") is False, "verified report has urlLeak=false")
        report_check(checks, verified_summary.get("mode") == "smoke", "verified source summary mode is smoke")
        report_check(checks, verified_summary.get("ok") is True, "verified source summary ok=true")
        report_check(checks, verified_summary.get("confirmed") is True, "verified source summary confirmed=true")
        report_check(checks, verified_summary.get("mutated") is True, "verified source summary mutated=true")
        report_check(checks, verified_summary.get("urlLeak") is False, "verified source summary urlLeak=false")

    if preflight is not None and smoke is not None:
        report_check(checks, preflight.get("bin") == smoke.get("bin"), "preflight and smoke used the same binary")
        report_check(
            checks,
            report_field(preflight, "binary", "sha256") == report_field(smoke, "binary", "sha256"),
            "preflight and smoke binary SHA256 match",
        )
        report_check(
            checks,
            report_field(preflight, "desktopSession", "user") == report_field(smoke, "desktopSession", "user"),
            "preflight and smoke ran as the same desktop user",
        )

    if smoke is not None and verified is not None:
        verified_summary = verified.get("sourceSummary") if isinstance(verified.get("sourceSummary"), dict) else {}
        for key in ("schemaVersion", "mode", "ok", "confirmed", "mutated", "urlLeak", "rootUser"):
            report_check(
                checks,
                verified_summary.get(key) == smoke.get(key),
                f"verified source summary matches smoke {key}",
            )
        report_check(
            checks,
            report_field(verified_summary, "binary", "sha256") == report_field(smoke, "binary", "sha256"),
            "verified source summary matches smoke binary SHA256",
        )
        report_check(
            checks,
            report_field(verified_summary, "desktopSession", "looksLikeDesktopSession")
            == report_field(smoke, "desktopSession", "looksLikeDesktopSession"),
            "verified source summary matches smoke desktop session",
        )
        if smoke_verification is not None:
            report_check(
                checks,
                verified.get("ok") == smoke_verification.get("ok"),
                "verified report result matches fresh smoke verification",
            )

    verification["ok"] = all(check["ok"] for check in checks)
    return verification


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Run a real GNOME system proxy smoke test. This temporarily changes "
            "the current desktop user's proxy settings and restores the original "
            "gsettings values before exit."
        )
    )
    parser.add_argument(
        "--bin",
        default=os.environ.get(DEFAULT_BIN_ENV, "clash-tui"),
        help=f"clash-tui binary path (default: ${DEFAULT_BIN_ENV} or PATH lookup)",
    )
    parser.add_argument(
        "--yes",
        action="store_true",
        help=f"confirm the desktop proxy mutation; alternatively set {CONFIRM_ENV}=1",
    )
    parser.add_argument(
        "--preflight",
        action="store_true",
        help="run only read-only environment checks; does not require confirmation and does not mutate proxy settings",
    )
    parser.add_argument(
        "--verify-report",
        metavar="PATH",
        help="verify an existing final smoke JSON report without mutating proxy settings",
    )
    parser.add_argument(
        "--verify-acceptance-dir",
        metavar="DIR",
        help="verify an acceptance output directory with preflight, smoke, and verified JSON reports",
    )
    parser.add_argument(
        "--allow-root",
        action="store_true",
        help="allow running as root; normally this should run as the logged-in desktop user",
    )
    parser.add_argument(
        "--output",
        help="also write the final JSON report to this path using an atomic replace",
    )
    return parser


def main() -> int:
    args = build_parser().parse_args()
    if args.verify_acceptance_dir:
        report = verify_acceptance_dir(args.verify_acceptance_dir)
        return finish_report(report, args.output)
    if args.verify_report:
        report = verify_report_file(args.verify_report)
        return finish_report(report, args.output)
    if args.preflight:
        report = preflight_report(args.bin, args.allow_root)
        return finish_report(report, args.output)

    binary = binary_report(args.bin)
    report: dict[str, Any] = {
        "schemaVersion": REPORT_SCHEMA_VERSION,
        "ok": False,
        "mode": "smoke",
        "bin": args.bin,
        "confirmed": args.yes or os.environ.get(CONFIRM_ENV) == "1",
        "rootUser": is_root_user(),
        "allowRoot": args.allow_root,
        "binaryAvailable": bool(binary["available"]),
        "binary": binary,
        "desktopSession": desktop_session_report(),
        "gsettings": gsettings_schema_report(),
        "mutated": False,
        "checks": [],
        "restore": {},
    }
    original_gsettings: dict[str, dict[str, str]] | None = None
    initial_app_enabled = False
    app_state_known = False

    try:
        if not report["confirmed"]:
            fail(f"confirmation required: pass --yes or set {CONFIRM_ENV}=1")
        if report["rootUser"] and not args.allow_root:
            fail("run this smoke as the logged-in desktop user, not root")
        if not report["binaryAvailable"]:
            fail(f"binary not found: {args.bin}")

        require_schema()
        original_gsettings = snapshot_gsettings()
        report["beforeGsettings"] = original_gsettings

        before_status = cli_json(args.bin, ["system-proxy", "status"])
        report["beforeStatus"] = before_status
        if before_status["code"] != 0:
            fail(f"system-proxy status failed: {before_status['stderr']}")
        initial_app_enabled = bool(before_status["data"].get("enabled"))
        app_state_known = True

        doctor = cli_json(args.bin, ["system-proxy", "doctor"])
        report["doctor"] = doctor
        if doctor["code"] != 0:
            fail(f"system-proxy doctor failed: {doctor['stderr']}")
        require(
            bool(doctor["data"].get("canAutoApply")),
            "system-proxy doctor reports canAutoApply=true",
            report["checks"],
        )

        endpoint = doctor["data"].get("endpoint", {})
        on = cli_json(args.bin, ["system-proxy", "on"])
        report["on"] = on
        report["mutated"] = True
        require(on["code"] == 0, "system-proxy on exits with code 0", report["checks"])
        require(bool(on["data"].get("enabled")), "system-proxy on reports enabled=true", report["checks"])
        require(bool(on["data"].get("configSaved")), "system-proxy on reports configSaved=true", report["checks"])
        require(on["data"].get("platformApplied") is True, "system-proxy on reports platformApplied=true", report["checks"])

        after_on = snapshot_gsettings()
        report["afterOnGsettings"] = after_on
        validate_after_on(after_on, endpoint, report["checks"])

        off = cli_json(args.bin, ["system-proxy", "off"])
        report["off"] = off
        require(off["code"] == 0, "system-proxy off exits with code 0", report["checks"])
        require(bool(off["data"].get("configSaved")), "system-proxy off reports configSaved=true", report["checks"])
        require(off["data"].get("platformApplied") is True, "system-proxy off reports platformApplied=true", report["checks"])

        after_off = snapshot_gsettings()
        report["afterOffGsettings"] = after_off
        validate_after_off(after_off, report["checks"])
        report["ok"] = True
    except SmokeError as err:
        report["error"] = str(err)
    finally:
        if original_gsettings is not None:
            app_restore_args = ["system-proxy", "on"] if initial_app_enabled else ["system-proxy", "off"]
            app_restore = cli_json(args.bin, app_restore_args) if app_state_known else None
            gsettings_restore = restore_gsettings(original_gsettings)
            report["restore"] = {
                "initialAppEnabled": initial_app_enabled,
                "appStateKnown": app_state_known,
                "appRestoreCommand": app_restore_args if app_state_known else None,
                "appRestore": app_restore,
                "gsettings": gsettings_restore,
            }
            app_restore_ok = (not app_state_known) or (app_restore is not None and app_restore["code"] == 0)
            report["ok"] = bool(report["ok"] and gsettings_restore["ok"] and app_restore_ok)

    return finish_report(report, args.output)


if __name__ == "__main__":
    sys.exit(main())
