#!/usr/bin/env python3
from __future__ import annotations

import contextlib
import hashlib
import io
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest


MODULE_PATH = Path(__file__).with_name("clash-tui-system-proxy-gnome-smoke.py")


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def binary_report(sha: str = "a" * 64):
    return {
        "input": "clash-tui",
        "available": True,
        "resolvedPath": "/opt/clash-tui/clash-tui",
        "sha256": sha,
    }


def load_smoke_module():
    spec = importlib.util.spec_from_file_location("clash_tui_system_proxy_gnome_smoke", MODULE_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"failed to load {MODULE_PATH}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def proxy_snapshot(mode: str = "'manual'", host: str = "'127.0.0.1'", port: str = "uint32 7897"):
    return {
        "org.gnome.system.proxy mode": {
            "schema": "org.gnome.system.proxy",
            "key": "mode",
            "value": mode,
        },
        "org.gnome.system.proxy ignore-hosts": {
            "schema": "org.gnome.system.proxy",
            "key": "ignore-hosts",
            "value": "['localhost', '127.0.0.1', '::1']",
        },
        "org.gnome.system.proxy.http host": {
            "schema": "org.gnome.system.proxy.http",
            "key": "host",
            "value": host,
        },
        "org.gnome.system.proxy.http port": {
            "schema": "org.gnome.system.proxy.http",
            "key": "port",
            "value": port,
        },
        "org.gnome.system.proxy.https host": {
            "schema": "org.gnome.system.proxy.https",
            "key": "host",
            "value": host,
        },
        "org.gnome.system.proxy.https port": {
            "schema": "org.gnome.system.proxy.https",
            "key": "port",
            "value": port,
        },
        "org.gnome.system.proxy.socks host": {
            "schema": "org.gnome.system.proxy.socks",
            "key": "host",
            "value": host,
        },
        "org.gnome.system.proxy.socks port": {
            "schema": "org.gnome.system.proxy.socks",
            "key": "port",
            "value": port,
        },
    }


def desktop_session(looks_like: bool = True):
    return {
        "uid": 501,
        "euid": 501,
        "user": "tester",
        "dbusSessionBusAddressPresent": looks_like,
        "dbusSessionBusAddressKind": "unix-path" if looks_like else None,
        "desktopMarkers": {"DISPLAY": ":0"} if looks_like else {},
        "desktopMarkerPresent": looks_like,
        "looksLikeDesktopSession": looks_like,
    }


def successful_smoke_report():
    before = proxy_snapshot(mode="'none'")
    after_on = proxy_snapshot(mode="'manual'")
    after_off = proxy_snapshot(mode="'none'")
    return {
        "schemaVersion": 2,
        "ok": True,
        "mode": "smoke",
        "bin": "clash-tui",
        "confirmed": True,
        "rootUser": False,
        "allowRoot": False,
        "binaryAvailable": True,
        "binary": binary_report(),
        "desktopSession": desktop_session(True),
        "gsettings": {
            "gsettingsPath": "/usr/bin/gsettings",
            "schemaAvailable": True,
            "listSchemasCode": 0,
            "message": "org.gnome.system.proxy schema is available",
        },
        "mutated": True,
        "urlLeak": False,
        "beforeGsettings": before,
        "doctor": {
            "code": 0,
            "stderr": "",
            "data": {
                "canAutoApply": True,
                "endpoint": {
                    "host": "127.0.0.1",
                    "port": 7897,
                    "bypass": "localhost,127.0.0.1,::1",
                },
            },
        },
        "on": {
            "code": 0,
            "stderr": "",
            "data": {
                "enabled": True,
                "configSaved": True,
                "platformApplied": True,
            },
        },
        "afterOnGsettings": after_on,
        "off": {
            "code": 0,
            "stderr": "",
            "data": {
                "configSaved": True,
                "platformApplied": True,
            },
        },
        "afterOffGsettings": after_off,
        "restore": {
            "initialAppEnabled": False,
            "appStateKnown": True,
            "appRestore": {
                "code": 0,
                "stderr": "",
                "data": {
                    "enabled": False,
                    "configSaved": True,
                },
            },
            "gsettings": {
                "ok": True,
                "matchesOriginal": True,
                "after": before,
            },
        },
    }


def successful_preflight_report():
    return {
        "schemaVersion": 2,
        "ok": True,
        "mode": "preflight",
        "bin": "clash-tui",
        "rootUser": False,
        "allowRoot": False,
        "binaryAvailable": True,
        "binary": binary_report(),
        "desktopSession": desktop_session(True),
        "gsettings": {
            "gsettingsPath": "/usr/bin/gsettings",
            "schemaAvailable": True,
            "listSchemasCode": 0,
            "message": "org.gnome.system.proxy schema is available",
        },
        "doctor": {
            "code": 0,
            "stderr": "",
            "data": {
                "canAutoApply": True,
                "endpoint": {
                    "host": "127.0.0.1",
                    "port": 7897,
                    "bypass": "localhost,127.0.0.1,::1",
                },
            },
        },
        "status": {
            "code": 0,
            "stderr": "",
            "data": {
                "enabled": False,
            },
        },
        "checks": [{"ok": True, "message": "preflight passed"}],
        "mutated": False,
        "urlLeak": False,
    }


class GnomeSmokeScriptTests(unittest.TestCase):
    def setUp(self):
        self.smoke = load_smoke_module()

    def test_parse_gsettings_variants(self):
        self.assertEqual(self.smoke.parse_variant_string("'manual'"), "manual")
        self.assertEqual(self.smoke.parse_variant_string("manual"), "manual")
        self.assertEqual(self.smoke.parse_variant_int("uint32 7897"), 7897)
        self.assertEqual(self.smoke.parse_variant_int("7897"), 7897)
        self.assertEqual(
            self.smoke.parse_variant_string_array("['localhost', '127.0.0.1']"),
            ["localhost", "127.0.0.1"],
        )

    def test_validate_after_on_accepts_expected_gsettings(self):
        checks = []
        self.smoke.validate_after_on(
            proxy_snapshot(),
            {"host": "127.0.0.1", "port": 7897, "bypass": "localhost,127.0.0.1,::1"},
            checks,
        )

        self.assertGreaterEqual(len(checks), 9)
        self.assertTrue(all(check["ok"] for check in checks))

    def test_validate_after_on_keeps_failed_check_for_report(self):
        checks = []

        with self.assertRaises(self.smoke.SmokeError):
            self.smoke.validate_after_on(
                proxy_snapshot(host="'0.0.0.0'"),
                {"host": "127.0.0.1", "port": 7897, "bypass": "localhost"},
                checks,
            )

        self.assertTrue(checks)
        self.assertFalse(checks[-1]["ok"])
        self.assertIn("host matches 127.0.0.1", checks[-1]["message"])

    def test_validate_after_off_requires_none_mode(self):
        checks = []
        self.smoke.validate_after_off(proxy_snapshot(mode="'none'"), checks)

        self.assertEqual(checks, [{"ok": True, "message": "GNOME proxy mode is none after system-proxy off"}])

    def test_restore_gsettings_writes_mode_last(self):
        calls = []
        original_set = self.smoke.gsettings_set
        original_snapshot = self.smoke.snapshot_gsettings
        snapshot = proxy_snapshot(mode="'manual'")

        try:
            self.smoke.gsettings_set = lambda schema, key, value: calls.append((schema, key, value)) or {
                "code": 0,
                "stderr": "",
            }
            self.smoke.snapshot_gsettings = lambda: snapshot

            result = self.smoke.restore_gsettings(snapshot)
        finally:
            self.smoke.gsettings_set = original_set
            self.smoke.snapshot_gsettings = original_snapshot

        self.assertTrue(result["ok"])
        self.assertEqual(calls[-1], ("org.gnome.system.proxy", "mode", "'manual'"))

    def test_run_reports_missing_command_without_throwing(self):
        result = self.smoke.run(["/definitely/missing/clash-tui"])

        self.assertEqual(result["code"], 127)
        self.assertIn("/definitely/missing/clash-tui", result["stderr"])

    def test_preflight_report_is_read_only_and_can_pass(self):
        originals = (
            self.smoke.binary_report,
            self.smoke.is_root_user,
            self.smoke.desktop_session_report,
            self.smoke.gsettings_schema_report,
            self.smoke.snapshot_gsettings,
            self.smoke.cli_json,
        )

        def fake_cli_json(_bin_path, args):
            if args == ["system-proxy", "doctor"]:
                data = {"canAutoApply": True}
            else:
                data = {"enabled": False}
            return {"code": 0, "stderr": "", "json": {"data": data}, "data": data}

        try:
            self.smoke.binary_report = lambda _bin_path: binary_report()
            self.smoke.is_root_user = lambda: False
            self.smoke.desktop_session_report = desktop_session
            self.smoke.gsettings_schema_report = lambda: {
                "gsettingsPath": "/usr/bin/gsettings",
                "schemaAvailable": True,
                "listSchemasCode": 0,
                "message": "schema available",
            }
            self.smoke.snapshot_gsettings = proxy_snapshot
            self.smoke.cli_json = fake_cli_json

            report = self.smoke.preflight_report("clash-tui", allow_root=False)
        finally:
            (
                self.smoke.binary_report,
                self.smoke.is_root_user,
                self.smoke.desktop_session_report,
                self.smoke.gsettings_schema_report,
                self.smoke.snapshot_gsettings,
                self.smoke.cli_json,
            ) = originals

        self.assertTrue(report["ok"])
        self.assertEqual(report["mode"], "preflight")
        self.assertFalse(report["mutated"])
        self.assertIn("gsettingsSnapshot", report)
        self.assertEqual(report["schemaVersion"], self.smoke.REPORT_SCHEMA_VERSION)
        self.assertTrue(report["desktopSession"]["looksLikeDesktopSession"])

    def test_preflight_report_requires_non_root_by_default(self):
        originals = (
            self.smoke.binary_report,
            self.smoke.is_root_user,
            self.smoke.desktop_session_report,
            self.smoke.gsettings_schema_report,
            self.smoke.snapshot_gsettings,
            self.smoke.cli_json,
        )

        try:
            self.smoke.binary_report = lambda _bin_path: binary_report()
            self.smoke.is_root_user = lambda: True
            self.smoke.desktop_session_report = desktop_session
            self.smoke.gsettings_schema_report = lambda: {
                "gsettingsPath": "/usr/bin/gsettings",
                "schemaAvailable": True,
                "listSchemasCode": 0,
                "message": "schema available",
            }
            self.smoke.snapshot_gsettings = proxy_snapshot
            self.smoke.cli_json = lambda _bin_path, args: {
                "code": 0,
                "stderr": "",
                "json": {"data": {"canAutoApply": True}},
                "data": {"canAutoApply": True},
            }

            report = self.smoke.preflight_report("clash-tui", allow_root=False)
        finally:
            (
                self.smoke.binary_report,
                self.smoke.is_root_user,
                self.smoke.desktop_session_report,
                self.smoke.gsettings_schema_report,
                self.smoke.snapshot_gsettings,
                self.smoke.cli_json,
            ) = originals

        self.assertFalse(report["ok"])
        self.assertTrue(report["rootUser"])
        self.assertFalse(report["checks"][1]["ok"])

    def test_preflight_report_requires_desktop_session_markers(self):
        originals = (
            self.smoke.binary_report,
            self.smoke.is_root_user,
            self.smoke.desktop_session_report,
            self.smoke.gsettings_schema_report,
            self.smoke.snapshot_gsettings,
            self.smoke.cli_json,
        )

        try:
            self.smoke.binary_report = lambda _bin_path: binary_report()
            self.smoke.is_root_user = lambda: False
            self.smoke.desktop_session_report = lambda: desktop_session(False)
            self.smoke.gsettings_schema_report = lambda: {
                "gsettingsPath": "/usr/bin/gsettings",
                "schemaAvailable": True,
                "listSchemasCode": 0,
                "message": "schema available",
            }
            self.smoke.snapshot_gsettings = proxy_snapshot
            self.smoke.cli_json = lambda _bin_path, args: {
                "code": 0,
                "stderr": "",
                "json": {"data": {"canAutoApply": True}},
                "data": {"canAutoApply": True},
            }

            report = self.smoke.preflight_report("clash-tui", allow_root=False)
        finally:
            (
                self.smoke.binary_report,
                self.smoke.is_root_user,
                self.smoke.desktop_session_report,
                self.smoke.gsettings_schema_report,
                self.smoke.snapshot_gsettings,
                self.smoke.cli_json,
            ) = originals

        self.assertFalse(report["ok"])
        self.assertFalse(report["desktopSession"]["looksLikeDesktopSession"])
        self.assertFalse(report["checks"][2]["ok"])
        self.assertIn("DBus session", report["checks"][2]["message"])

    def test_desktop_session_report_records_safe_markers(self):
        old_env = {
            key: os.environ.get(key)
            for key in (
                "DBUS_SESSION_BUS_ADDRESS",
                "DISPLAY",
                "WAYLAND_DISPLAY",
                "XDG_CURRENT_DESKTOP",
                "DESKTOP_SESSION",
            )
        }
        try:
            os.environ["DBUS_SESSION_BUS_ADDRESS"] = "unix:path=/run/user/501/bus"
            os.environ["DISPLAY"] = ":0"
            os.environ["XDG_CURRENT_DESKTOP"] = "GNOME"
            os.environ.pop("WAYLAND_DISPLAY", None)
            os.environ.pop("DESKTOP_SESSION", None)

            report = self.smoke.desktop_session_report()
        finally:
            for key, value in old_env.items():
                if value is None:
                    os.environ.pop(key, None)
                else:
                    os.environ[key] = value

        self.assertTrue(report["looksLikeDesktopSession"])
        self.assertEqual(report["dbusSessionBusAddressKind"], "unix-path")
        self.assertEqual(report["desktopMarkers"]["DISPLAY"], ":0")
        self.assertEqual(report["desktopMarkers"]["XDG_CURRENT_DESKTOP"], "GNOME")

    def test_finish_report_writes_output_json(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            output_path = Path(temp_dir) / "reports" / "gnome-preflight.json"
            report = {
                "ok": True,
                "mode": "preflight",
                "checks": [],
                "mutated": False,
            }

            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout):
                code = self.smoke.finish_report(report, str(output_path))

            self.assertEqual(code, 0)
            self.assertTrue(output_path.exists())
            from_stdout = json.loads(stdout.getvalue())
            from_file = json.loads(output_path.read_text(encoding="utf-8"))
            self.assertEqual(from_stdout, from_file)
            self.assertTrue(from_file["output"]["ok"])
            self.assertFalse(from_file["urlLeak"])
            self.assertEqual(from_file["nextSteps"][0]["code"], "run-confirmed-smoke")
            self.assertIn("CLASH_TUI_SYSTEM_PROXY_SMOKE=1", from_file["nextSteps"][0]["command"])

    def test_finish_report_marks_output_write_failure(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            output_path = Path(temp_dir)
            report = {
                "ok": True,
                "mode": "preflight",
                "checks": [],
                "mutated": False,
            }

            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout):
                code = self.smoke.finish_report(report, str(output_path))

            emitted = json.loads(stdout.getvalue())
            self.assertEqual(code, 1)
            self.assertFalse(emitted["ok"])
            self.assertFalse(emitted["output"]["ok"])
            self.assertIn("error", emitted["output"])
            self.assertIn("fix-output-path", [step["code"] for step in emitted["nextSteps"]])

    def test_next_steps_for_root_preflight_failure(self):
        report = {
            "ok": False,
            "mode": "preflight",
            "bin": "clash-tui",
            "rootUser": True,
            "allowRoot": False,
            "binaryAvailable": True,
            "desktopSession": desktop_session(False),
            "gsettings": {
                "schemaAvailable": True,
            },
            "doctor": {
                "data": {
                    "canAutoApply": False,
                }
            },
            "checks": [],
            "mutated": False,
        }

        steps = self.smoke.next_steps_for_report(report)
        codes = [step["code"] for step in steps]

        self.assertIn("use-desktop-user", codes)
        self.assertIn("use-desktop-session", codes)
        self.assertIn("fix-doctor-checks", codes)
        self.assertIn("clash-tui --json system-proxy doctor", steps[-1]["command"])

    def test_next_steps_for_unconfirmed_smoke(self):
        report = {
            "ok": False,
            "mode": "smoke",
            "bin": "/opt/clash-tui-fixture/bin/clash-tui",
            "confirmed": False,
            "mutated": False,
            "error": "confirmation required",
            "checks": [],
        }

        steps = self.smoke.next_steps_for_report(report)

        self.assertEqual(steps[0]["code"], "confirm-mutation")
        self.assertIn("--preflight", steps[0]["command"])
        self.assertIn("/opt/clash-tui-fixture/bin/clash-tui", steps[0]["command"])

    def test_next_steps_use_invoked_package_script_path(self):
        original_argv = sys.argv
        try:
            sys.argv = ["tools/clash-tui-system-proxy-gnome-smoke.py"]
            steps = self.smoke.next_steps_for_report(
                {
                    "ok": True,
                    "mode": "preflight",
                    "bin": "./clash-tui",
                    "checks": [],
                    "mutated": False,
                }
            )
        finally:
            sys.argv = original_argv

        self.assertEqual(steps[0]["code"], "run-confirmed-smoke")
        self.assertIn("tools/clash-tui-system-proxy-gnome-smoke.py", steps[0]["command"])

    def test_verify_smoke_report_accepts_complete_confirmed_report(self):
        verification = self.smoke.verify_smoke_report(successful_smoke_report(), "gnome-smoke.json")

        self.assertTrue(verification["ok"])
        self.assertEqual(verification["mode"], "verify-report")
        self.assertEqual(verification["sourceSummary"]["mode"], "smoke")
        self.assertTrue(verification["sourceSummary"]["desktopSession"]["looksLikeDesktopSession"])
        self.assertTrue(all(check["ok"] for check in verification["checks"]))

    def test_verify_smoke_report_rejects_preflight_report(self):
        report = successful_smoke_report()
        report.update({
            "ok": False,
            "mode": "preflight",
            "confirmed": None,
            "mutated": False,
            "desktopSession": desktop_session(False),
        })

        verification = self.smoke.verify_smoke_report(report, "gnome-preflight.json")
        failed = [check["message"] for check in verification["checks"] if not check["ok"]]

        self.assertFalse(verification["ok"])
        self.assertIn("source report mode is smoke", failed)
        self.assertIn("source report mutated=true", failed)
        self.assertIn("source report ran in a desktop session", failed)

    def test_verify_report_file_and_finish_report_write_verified_json(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            report_path = Path(temp_dir) / "gnome-smoke.json"
            output_path = Path(temp_dir) / "gnome-smoke-verified.json"
            report_path.write_text(json.dumps(successful_smoke_report()), encoding="utf-8")

            verification = self.smoke.verify_report_file(str(report_path))
            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout):
                code = self.smoke.finish_report(verification, str(output_path))

            self.assertEqual(code, 0)
            emitted = json.loads(stdout.getvalue())
            written = json.loads(output_path.read_text(encoding="utf-8"))
            self.assertEqual(emitted, written)
            self.assertTrue(written["ok"])
            self.assertFalse(written["mutated"])
            self.assertFalse(written["urlLeak"])
            self.assertEqual(written["nextSteps"][0]["code"], "archive-verified-report")

    def test_main_verify_report_mode_is_read_only(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            report_path = Path(temp_dir) / "gnome-smoke.json"
            output_path = Path(temp_dir) / "gnome-smoke-verified.json"
            report_path.write_text(json.dumps(successful_smoke_report()), encoding="utf-8")
            old_argv = sys.argv
            try:
                sys.argv = [
                    "clash-tui-system-proxy-gnome-smoke.py",
                    "--verify-report",
                    str(report_path),
                    "--output",
                    str(output_path),
                ]
                stdout = io.StringIO()
                with contextlib.redirect_stdout(stdout):
                    code = self.smoke.main()
            finally:
                sys.argv = old_argv

            emitted = json.loads(stdout.getvalue())
            self.assertEqual(code, 0)
            self.assertTrue(output_path.exists())
            self.assertEqual(emitted["mode"], "verify-report")
            self.assertTrue(emitted["ok"])
            self.assertFalse(emitted["mutated"])

    def write_acceptance_reports(self, temp_dir: Path):
        preflight_path = temp_dir / "gnome-preflight.json"
        smoke_path = temp_dir / "gnome-smoke.json"
        verified_path = temp_dir / "gnome-smoke-verified.json"
        smoke = successful_smoke_report()
        preflight_path.write_text(json.dumps(successful_preflight_report()), encoding="utf-8")
        smoke_path.write_text(json.dumps(smoke), encoding="utf-8")
        verification = self.smoke.verify_smoke_report(smoke, str(smoke_path))
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            code = self.smoke.finish_report(verification, str(verified_path))
        self.assertEqual(code, 0)
        return preflight_path, smoke_path, verified_path

    def test_verify_acceptance_dir_accepts_complete_bundle(self):
        with tempfile.TemporaryDirectory() as raw:
            temp_dir = Path(raw)
            preflight_path, smoke_path, verified_path = self.write_acceptance_reports(temp_dir)

            verification = self.smoke.verify_acceptance_dir(str(temp_dir))

            self.assertTrue(verification["ok"])
            self.assertEqual(verification["mode"], "verify-acceptance")
            self.assertFalse(verification["mutated"])
            self.assertEqual(verification["reportHashes"]["preflight"]["sha256"], sha256_file(preflight_path))
            self.assertEqual(verification["reportHashes"]["smoke"]["sha256"], sha256_file(smoke_path))
            self.assertEqual(verification["reportHashes"]["verified"]["sha256"], sha256_file(verified_path))
            self.assertEqual(verification["reportSummaries"]["preflight"]["mode"], "preflight")
            self.assertEqual(verification["reportSummaries"]["smoke"]["mode"], "smoke")
            self.assertTrue(verification["smokeVerification"]["ok"])
            self.assertTrue(all(check["ok"] for check in verification["checks"]))

    def test_verify_acceptance_dir_rejects_missing_verified_report(self):
        with tempfile.TemporaryDirectory() as raw:
            temp_dir = Path(raw)
            _, _, verified_path = self.write_acceptance_reports(temp_dir)
            verified_path.unlink()

            verification = self.smoke.verify_acceptance_dir(str(temp_dir))
            failed = [check["message"] for check in verification["checks"] if not check["ok"]]

            self.assertFalse(verification["ok"])
            self.assertIn("verified report can be read", failed)

    def test_verify_acceptance_dir_rejects_failed_preflight(self):
        with tempfile.TemporaryDirectory() as raw:
            temp_dir = Path(raw)
            self.write_acceptance_reports(temp_dir)
            preflight = successful_preflight_report()
            preflight["ok"] = False
            preflight["desktopSession"] = desktop_session(False)
            (temp_dir / "gnome-preflight.json").write_text(json.dumps(preflight), encoding="utf-8")

            verification = self.smoke.verify_acceptance_dir(str(temp_dir))
            failed = [check["message"] for check in verification["checks"] if not check["ok"]]

            self.assertFalse(verification["ok"])
            self.assertIn("preflight report ok=true", failed)
            self.assertIn("preflight ran in a desktop session", failed)

    def test_verify_acceptance_dir_rejects_binary_sha_mismatch(self):
        with tempfile.TemporaryDirectory() as raw:
            temp_dir = Path(raw)
            self.write_acceptance_reports(temp_dir)
            preflight = successful_preflight_report()
            preflight["binary"] = binary_report("b" * 64)
            (temp_dir / "gnome-preflight.json").write_text(json.dumps(preflight), encoding="utf-8")

            verification = self.smoke.verify_acceptance_dir(str(temp_dir))
            failed = [check["message"] for check in verification["checks"] if not check["ok"]]

            self.assertFalse(verification["ok"])
            self.assertIn("preflight and smoke binary SHA256 match", failed)

    def test_main_verify_acceptance_dir_writes_report(self):
        with tempfile.TemporaryDirectory() as raw:
            temp_dir = Path(raw)
            output_path = temp_dir / "gnome-acceptance-verified.json"
            self.write_acceptance_reports(temp_dir)
            old_argv = sys.argv
            try:
                sys.argv = [
                    "clash-tui-system-proxy-gnome-smoke.py",
                    "--verify-acceptance-dir",
                    str(temp_dir),
                    "--output",
                    str(output_path),
                ]
                stdout = io.StringIO()
                with contextlib.redirect_stdout(stdout):
                    code = self.smoke.main()
            finally:
                sys.argv = old_argv

            emitted = json.loads(stdout.getvalue())
            written = json.loads(output_path.read_text(encoding="utf-8"))
            self.assertEqual(code, 0)
            self.assertEqual(emitted, written)
            self.assertTrue(written["ok"])
            self.assertEqual(written["mode"], "verify-acceptance")
            self.assertFalse(written["mutated"])
            self.assertEqual(written["nextSteps"][0]["code"], "archive-acceptance-report")


if __name__ == "__main__":
    unittest.main()
