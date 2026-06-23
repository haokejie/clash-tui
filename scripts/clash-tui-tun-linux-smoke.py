#!/usr/bin/env python3
"""Smoke-test Linux TUN application for clash-tui.

The default preflight mode is read-only. The confirmed smoke temporarily runs
`tun on`, starts the core, waits for the Meta link and TUN route, then runs
`tun off` and `core stop` in a finally block.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from typing import Any


CONFIRM_ENV = "CLASH_TUI_TUN_SMOKE"
DEFAULT_BIN_ENV = "CLASH_TUI_BIN"
TUN_LINK_NAME = "Meta"
TUN_ROUTE = "198.18.0.0/30"


class SmokeError(RuntimeError):
    pass


def run(argv: list[str], timeout: int = 30) -> dict[str, Any]:
    try:
        completed = subprocess.run(argv, text=True, capture_output=True, timeout=timeout, check=False)
    except OSError as err:
        return {
            "argv": argv,
            "code": 127,
            "stdout": "",
            "stderr": str(err),
        }
    except subprocess.TimeoutExpired as err:
        return {
            "argv": argv,
            "code": 124,
            "stdout": err.stdout or "",
            "stderr": err.stderr or f"timed out after {timeout}s",
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


def script_command() -> str:
    script_path = (
        sys.argv[0]
        if sys.argv and sys.argv[0].endswith("clash-tui-tun-linux-smoke.py")
        else "scripts/clash-tui-tun-linux-smoke.py"
    )
    return f"python3 {shlex.quote(script_path)}"


def next_steps_for_report(report: dict[str, Any]) -> list[dict[str, str]]:
    mode = str(report.get("mode", ""))
    bin_path = str(report.get("bin") or os.environ.get(DEFAULT_BIN_ENV, "clash-tui"))
    bin_arg = shlex.quote(bin_path)
    base_smoke = f"{CONFIRM_ENV}=1 {script_command()} --bin {bin_arg}"

    if report.get("ok"):
        if mode == "preflight":
            return [
                {
                    "code": "run-confirmed-smoke",
                    "message": "TUN 预检已通过；可在允许短暂创建 Meta 网卡和路由的 Linux 测试会话中执行确认式 smoke。",
                    "command": base_smoke,
                }
            ]
        if mode == "smoke":
            return [
                {
                    "code": "archive-report",
                    "message": "确认式 TUN smoke 已通过；保存此 JSON 作为 runtime、Meta link、TUN route 和恢复证据。",
                }
            ]

    steps: list[dict[str, str]] = []
    error = str(report.get("error") or "")

    if mode == "smoke" and not report.get("confirmed"):
        steps.append(
            {
                "code": "confirm-mutation",
                "message": "确认式 smoke 会短暂创建 Meta 网卡和 198.18.0.0/30 路由；先跑只读预检，通过后再设置确认环境变量或传 --yes。",
                "command": f"{script_command()} --preflight --bin {bin_arg}",
            }
        )

    if not report.get("linux", True):
        steps.append(
            {
                "code": "use-linux",
                "message": "TUN smoke 仅验证 Linux mihomo TUN；请在 Linux 验收机运行。",
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
        return steps

    if not report.get("binaryAvailable", True):
        steps.append(
            {
                "code": "fix-binary",
                "message": "未找到 clash-tui；请安装成品包、加入 PATH，或用 --bin 指向短命令/二进制。",
            }
        )

    if not report.get("ipAvailable", True):
        steps.append(
            {
                "code": "install-iproute2",
                "message": "未找到 ip 命令；请安装 iproute2 后重跑 TUN 预检或 smoke。",
            }
        )

    current_profile = report.get("currentProfile") if isinstance(report.get("currentProfile"), dict) else {}
    if current_profile and current_profile.get("code") != 0:
        steps.append(
            {
                "code": "import-profile",
                "message": "当前没有可用 Profile；请先通过 stdin 导入真实订阅并确认 proxy groups 非空，再停止 Core 后重跑预检。",
                "command": f"printf '%s\\n' '<subscription-url>' | {bin_arg} --json profile import-url --stdin --start-core",
            }
        )

    core_state = core_state_from_report(report)
    if core_state and core_state not in ("Stopped", "Crashed", "stopped", "crashed"):
        steps.append(
            {
                "code": "stop-core-first",
                "message": "为避免干扰正在运行的 Core，请先在测试窗口执行 core stop，确认当前 TUN 关闭后再跑 smoke。",
                "command": f"{bin_arg} --json core stop",
            }
        )

    tun_enabled = tun_enabled_from_report(report)
    before = report.get("before") if isinstance(report.get("before"), dict) else {}
    if tun_enabled or before.get("metaExists") or before.get("routeExists"):
        steps.append(
            {
                "code": "cleanup-existing-tun",
                "message": "当前已有 TUN 配置、Meta 网卡或 TUN 路由；请先执行 tun off 与 core stop，并确认 link/route 消失。",
                "command": f"{bin_arg} --json tun off && {bin_arg} --json core stop",
            }
        )

    doctor = report.get("doctor") if isinstance(report.get("doctor"), dict) else {}
    if "canEnable" in json.dumps(doctor, ensure_ascii=False) and not bool(doctor.get("data", {}).get("canEnable")):
        steps.append(
            {
                "code": "fix-doctor-checks",
                "message": "tun doctor 尚未允许开启；请按 doctor.checks/manualAction 修复 /dev/net/tun、权限或 CAP_NET_ADMIN 后重跑预检。",
                "command": f"{bin_arg} --json tun doctor",
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

    cleanup = report.get("cleanup") if isinstance(report.get("cleanup"), dict) else {}
    if mode == "smoke" and report.get("mutated") and not cleanup.get("ok", True):
        steps.append(
            {
                "code": "manual-restore-tun",
                "message": "脚本已尝试恢复但清理校验失败；请手动执行 tun off、core stop，并确认 Meta link 与 198.18.0.0/30 路由消失。",
                "command": f"{bin_arg} --json tun off && {bin_arg} --json core stop",
            }
        )

    if not steps:
        steps.append(
            {
                "code": "rerun-preflight",
                "message": "请根据 checks/error 修正环境后重跑只读预检；不要直接在未知网络状态执行确认式 TUN on/off。",
                "command": f"{script_command()} --preflight --bin {bin_arg}",
            }
        )

    return steps


def binary_exists(bin_path: str) -> bool:
    return shutil.which(bin_path) is not None or os.path.exists(bin_path)


def is_linux() -> bool:
    return platform.system().lower() == "linux"


def cli_json(bin_path: str, args: list[str], timeout: int = 30) -> dict[str, Any]:
    result = run([bin_path, "--json", *args], timeout=timeout)
    parsed: dict[str, Any] | None = None
    if result["stdout"].strip():
        try:
            parsed = json.loads(result["stdout"])
        except json.JSONDecodeError as err:
            parsed = None
            result["jsonError"] = str(err)
    return {
        "code": result["code"],
        "stderr": result["stderr"].strip(),
        "json": parsed,
        "data": parsed.get("data", {}) if isinstance(parsed, dict) else {},
    }


def ip_link_meta() -> dict[str, Any]:
    return run(["ip", "link", "show", TUN_LINK_NAME], timeout=10)


def tun_route() -> dict[str, Any]:
    return run(["ip", "route", "show", TUN_ROUTE], timeout=10)


def link_route_snapshot() -> dict[str, Any]:
    if shutil.which("ip") is None:
        return {
            "ipAvailable": False,
            "metaExists": False,
            "metaLine": "",
            "routeExists": False,
            "route": "",
        }
    link = ip_link_meta()
    route = tun_route()
    link_lines = (link.get("stdout") or "").splitlines()
    route_text = (route.get("stdout") or "").strip()
    return {
        "ipAvailable": True,
        "metaExists": link.get("code") == 0,
        "metaLine": link_lines[0] if link_lines else "",
        "routeExists": bool(route_text),
        "route": route_text,
    }


def load_runtime_tun_enabled(runtime_path: str | None) -> bool | None:
    if not runtime_path:
        return None
    path = Path(runtime_path)
    if not path.is_file():
        return None
    in_tun = False
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        if line.startswith("tun:"):
            in_tun = True
            continue
        if in_tun and line and not line.startswith(" "):
            return False
        if in_tun and line.strip() == "enable: true":
            return True
    return False


def core_state_from_report(report: dict[str, Any]) -> str | None:
    core_status = report.get("coreStatus") if isinstance(report.get("coreStatus"), dict) else {}
    data = core_status.get("data") if isinstance(core_status.get("data"), dict) else {}
    state = data.get("state")
    return str(state) if state is not None else None


def tun_enabled_from_report(report: dict[str, Any]) -> bool | None:
    tun_status = report.get("tunStatus") if isinstance(report.get("tunStatus"), dict) else {}
    data = tun_status.get("data") if isinstance(tun_status.get("data"), dict) else {}
    enabled = data.get("enabled")
    return bool(enabled) if enabled is not None else None


def wait_tun_ready(bin_path: str, runtime_path: str | None, timeout_secs: int) -> dict[str, Any]:
    end = time.time() + timeout_secs
    last: dict[str, Any] = {}
    while time.time() < end:
        core_status = cli_json(bin_path, ["core", "status"], timeout=10)
        link_route = link_route_snapshot()
        runtime_enabled = load_runtime_tun_enabled(runtime_path)
        last = {
            "coreStatus": core_status,
            "runtimeTunEnabled": runtime_enabled,
            **link_route,
        }
        if runtime_enabled and link_route["metaExists"] and link_route["routeExists"]:
            return last
        time.sleep(1)
    return last


def wait_tun_absent(timeout_secs: int) -> dict[str, Any]:
    end = time.time() + timeout_secs
    last: dict[str, Any] = {}
    while time.time() < end:
        last = link_route_snapshot()
        if not last["metaExists"] and not last["routeExists"]:
            return last
        time.sleep(1)
    return last


def preflight_report(bin_path: str) -> dict[str, Any]:
    checks: list[dict[str, Any]] = []
    linux = is_linux()
    bin_available = binary_exists(bin_path)
    ip_available = shutil.which("ip") is not None
    before = link_route_snapshot() if linux else {
        "ipAvailable": ip_available,
        "metaExists": False,
        "metaLine": "",
        "routeExists": False,
        "route": "",
    }
    report: dict[str, Any] = {
        "ok": False,
        "mode": "preflight",
        "bin": bin_path,
        "linux": linux,
        "binaryAvailable": bin_available,
        "ipAvailable": ip_available,
        "before": before,
        "checks": checks,
        "mutated": False,
    }

    checks.append({"ok": linux, "message": "running on Linux"})
    checks.append({"ok": bin_available, "message": "clash-tui binary is available"})
    checks.append({"ok": ip_available, "message": "ip command is available"})

    if bin_available:
        tun_status = cli_json(bin_path, ["tun", "status"])
        doctor = cli_json(bin_path, ["tun", "doctor"])
        core_status = cli_json(bin_path, ["core", "status"])
        current_profile = cli_json(bin_path, ["profile", "current"])
        report["tunStatus"] = tun_status
        report["doctor"] = doctor
        report["coreStatus"] = core_status
        report["currentProfile"] = current_profile
        checks.append({"ok": tun_status["code"] == 0, "message": "tun status exits with code 0"})
        checks.append({"ok": doctor["code"] == 0, "message": "tun doctor exits with code 0"})
        checks.append({"ok": core_status["code"] == 0, "message": "core status exits with code 0"})
        checks.append({"ok": current_profile["code"] == 0, "message": "current profile is available"})
        checks.append({
            "ok": bool(doctor["data"].get("canEnable")),
            "message": "tun doctor reports canEnable=true",
        })
        checks.append({
            "ok": not bool(tun_status["data"].get("enabled")),
            "message": "tun status is disabled before smoke",
        })
        state = str(core_status["data"].get("state", ""))
        checks.append({
            "ok": state in ("Stopped", "Crashed", "stopped", "crashed"),
            "message": "core is stopped before smoke",
        })

    checks.append({"ok": not before["metaExists"], "message": f"{TUN_LINK_NAME} link is absent before smoke"})
    checks.append({"ok": not before["routeExists"], "message": f"{TUN_ROUTE} route is absent before smoke"})

    report["ok"] = all(check["ok"] for check in checks)
    return report


def require(condition: bool, message: str, checks: list[dict[str, Any]]) -> None:
    checks.append({"ok": condition, "message": message})
    if not condition:
        fail(message)


def cleanup_tun(bin_path: str, timeout_secs: int) -> dict[str, Any]:
    close_connections = cli_json(bin_path, ["connections", "close-all"], timeout=15)
    tun_off = cli_json(bin_path, ["tun", "off"], timeout=45)
    core_stop = cli_json(bin_path, ["core", "stop"], timeout=45)
    absent = wait_tun_absent(timeout_secs)
    return {
        "ok": tun_off["code"] == 0 and core_stop["code"] == 0 and not absent["metaExists"] and not absent["routeExists"],
        "connectionsCloseAll": close_connections,
        "tunOff": tun_off,
        "coreStop": core_stop,
        "after": absent,
    }


def smoke_report(bin_path: str, confirmed: bool, timeout_secs: int) -> dict[str, Any]:
    report = preflight_report(bin_path)
    report["mode"] = "smoke"
    report["confirmed"] = confirmed
    report["timeoutSecs"] = timeout_secs
    report["mutated"] = False
    report["cleanup"] = {}

    try:
        if not confirmed:
            fail(f"confirmation required: pass --yes or set {CONFIRM_ENV}=1")
        if not report["ok"]:
            fail("preflight checks failed; run --preflight and fix checks before confirmed smoke")

        tun_on = cli_json(bin_path, ["tun", "on"], timeout=45)
        report["tunOn"] = tun_on
        report["mutated"] = True
        tun_data = tun_on["data"]
        runtime_path = tun_data.get("runtimePath")
        require(tun_on["code"] == 0, "tun on exits with code 0", report["checks"])
        require(bool(tun_data.get("enabled")), "tun on reports enabled=true", report["checks"])
        require(bool(tun_data.get("runtimeGenerated")), "tun on reports runtimeGenerated=true", report["checks"])

        core_start = cli_json(bin_path, ["core", "start"], timeout=60)
        report["coreStart"] = core_start
        require(core_start["code"] == 0, "core start exits with code 0", report["checks"])

        ready = wait_tun_ready(bin_path, runtime_path, timeout_secs)
        report["afterOn"] = ready
        require(ready.get("runtimeTunEnabled") is True, "runtime has tun.enable=true", report["checks"])
        require(bool(ready.get("metaExists")), f"{TUN_LINK_NAME} link exists after core start", report["checks"])
        require(bool(ready.get("routeExists")), f"{TUN_ROUTE} route exists after core start", report["checks"])

        cleanup = cleanup_tun(bin_path, timeout_secs)
        report["cleanup"] = cleanup
        require(cleanup["tunOff"]["code"] == 0, "tun off exits with code 0", report["checks"])
        require(cleanup["coreStop"]["code"] == 0, "core stop exits with code 0", report["checks"])
        require(not cleanup["after"]["metaExists"], f"{TUN_LINK_NAME} link is absent after cleanup", report["checks"])
        require(not cleanup["after"]["routeExists"], f"{TUN_ROUTE} route is absent after cleanup", report["checks"])
        report["ok"] = True
    except SmokeError as err:
        report["error"] = str(err)
    finally:
        if report.get("mutated") and not report.get("cleanup"):
            report["cleanup"] = cleanup_tun(bin_path, timeout_secs)
        elif report.get("mutated") and not (report.get("cleanup") or {}).get("ok", False):
            report["cleanupFinal"] = cleanup_tun(bin_path, timeout_secs)
            report["ok"] = bool(report.get("ok") and report["cleanupFinal"]["ok"])

    return report


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Run a Linux TUN smoke test. Preflight is read-only; confirmed smoke "
            "temporarily creates the Meta link and TUN route, then restores them."
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
        help=f"confirm TUN/Core mutation; alternatively set {CONFIRM_ENV}=1",
    )
    parser.add_argument(
        "--preflight",
        action="store_true",
        help="run only read-only environment checks; does not mutate TUN, Core, link, or routes",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=45,
        help="seconds to wait for Meta link / TUN route creation and cleanup (default: 45)",
    )
    parser.add_argument(
        "--output",
        help="also write the final JSON report to this path using an atomic replace",
    )
    return parser


def main() -> int:
    args = build_parser().parse_args()
    if args.preflight:
        report = preflight_report(args.bin)
    else:
        report = smoke_report(args.bin, args.yes or os.environ.get(CONFIRM_ENV) == "1", args.timeout)
    return finish_report(report, args.output)


if __name__ == "__main__":
    sys.exit(main())
