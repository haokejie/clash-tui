#!/usr/bin/env python3
from __future__ import annotations

import contextlib
import io
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest


MODULE_PATH = Path(__file__).with_name("clash-tui-tun-linux-smoke.py")


def load_tun_module():
    spec = importlib.util.spec_from_file_location("clash_tui_tun_linux_smoke", MODULE_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"failed to load {MODULE_PATH}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class TunLinuxSmokeScriptTests(unittest.TestCase):
    def setUp(self):
        self.smoke = load_tun_module()

    def test_load_runtime_tun_enabled(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            runtime = Path(temp_dir) / "clash.yaml"
            runtime.write_text(
                "\n".join(
                    [
                        "mixed-port: 7897",
                        "tun:",
                        "  enable: true",
                        "  stack: mixed",
                        "proxies: []",
                    ]
                ),
                encoding="utf-8",
            )

            self.assertTrue(self.smoke.load_runtime_tun_enabled(str(runtime)))

    def test_load_runtime_tun_enabled_returns_false_when_tun_section_ends(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            runtime = Path(temp_dir) / "clash.yaml"
            runtime.write_text("tun:\n  stack: mixed\nproxies: []\n", encoding="utf-8")

            self.assertFalse(self.smoke.load_runtime_tun_enabled(str(runtime)))

    def test_finish_report_writes_next_steps_and_output_json(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            output_path = Path(temp_dir) / "reports" / "tun-preflight.json"
            report = {
                "ok": True,
                "mode": "preflight",
                "bin": "clash-tui",
                "checks": [],
                "mutated": False,
            }

            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout):
                code = self.smoke.finish_report(report, str(output_path))

            self.assertEqual(code, 0)
            from_stdout = json.loads(stdout.getvalue())
            from_file = json.loads(output_path.read_text(encoding="utf-8"))
            self.assertEqual(from_stdout, from_file)
            self.assertEqual(from_file["nextSteps"][0]["code"], "run-confirmed-smoke")
            self.assertIn("CLASH_TUI_TUN_SMOKE=1", from_file["nextSteps"][0]["command"])
            self.assertFalse(from_file["urlLeak"])

    def test_finish_report_marks_output_write_failure(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            report = {
                "ok": True,
                "mode": "preflight",
                "checks": [],
                "mutated": False,
            }

            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout):
                code = self.smoke.finish_report(report, temp_dir)

            emitted = json.loads(stdout.getvalue())
            self.assertEqual(code, 1)
            self.assertFalse(emitted["ok"])
            self.assertFalse(emitted["output"]["ok"])
            self.assertIn("fix-output-path", [step["code"] for step in emitted["nextSteps"]])

    def test_next_steps_for_unconfirmed_smoke(self):
        steps = self.smoke.next_steps_for_report(
            {
                "ok": False,
                "mode": "smoke",
                "bin": "/opt/clash-tui-fixture/clash-tui",
                "confirmed": False,
                "mutated": False,
                "error": "confirmation required",
            }
        )

        self.assertEqual(steps[0]["code"], "confirm-mutation")
        self.assertIn("--preflight", steps[0]["command"])
        self.assertIn("/opt/clash-tui-fixture/clash-tui", steps[0]["command"])

    def test_next_steps_for_non_linux_stays_focused(self):
        steps = self.smoke.next_steps_for_report(
            {
                "ok": False,
                "mode": "preflight",
                "bin": "clash-tui",
                "linux": False,
                "binaryAvailable": True,
                "ipAvailable": False,
                "doctor": {
                    "data": {
                        "canEnable": False,
                    }
                },
            }
        )

        self.assertEqual([step["code"] for step in steps], ["use-linux"])

    def test_next_steps_require_profile_and_cleanup_existing_tun(self):
        report = {
            "ok": False,
            "mode": "preflight",
            "bin": "clash-tui",
            "linux": True,
            "binaryAvailable": True,
            "ipAvailable": True,
            "currentProfile": {
                "code": 1,
            },
            "tunStatus": {
                "data": {
                    "enabled": True,
                }
            },
            "coreStatus": {
                "data": {
                    "state": "Running",
                }
            },
            "before": {
                "metaExists": True,
                "routeExists": True,
            },
            "doctor": {
                "data": {
                    "canEnable": False,
                }
            },
        }

        codes = [step["code"] for step in self.smoke.next_steps_for_report(report)]

        self.assertIn("import-profile", codes)
        self.assertIn("stop-core-first", codes)
        self.assertIn("cleanup-existing-tun", codes)
        self.assertIn("fix-doctor-checks", codes)

    def test_next_steps_use_invoked_package_script_path(self):
        original_argv = sys.argv
        try:
            sys.argv = ["tools/clash-tui-tun-linux-smoke.py"]
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
        self.assertIn("tools/clash-tui-tun-linux-smoke.py", steps[0]["command"])

    def test_preflight_report_can_pass_with_mocks(self):
        originals = (
            self.smoke.is_linux,
            self.smoke.binary_exists,
            self.smoke.shutil.which,
            self.smoke.link_route_snapshot,
            self.smoke.cli_json,
        )

        def fake_cli_json(_bin_path, args):
            data = {}
            if args == ["tun", "status"]:
                data = {"enabled": False}
            elif args == ["tun", "doctor"]:
                data = {"canEnable": True}
            elif args == ["core", "status"]:
                data = {"state": "Stopped"}
            elif args == ["profile", "current"]:
                data = {"uid": "current"}
            return {"code": 0, "stderr": "", "json": {"data": data}, "data": data}

        try:
            self.smoke.is_linux = lambda: True
            self.smoke.binary_exists = lambda _bin_path: True
            self.smoke.shutil.which = lambda name: "/sbin/ip" if name == "ip" else None
            self.smoke.link_route_snapshot = lambda: {
                "ipAvailable": True,
                "metaExists": False,
                "metaLine": "",
                "routeExists": False,
                "route": "",
            }
            self.smoke.cli_json = fake_cli_json

            report = self.smoke.preflight_report("clash-tui")
        finally:
            (
                self.smoke.is_linux,
                self.smoke.binary_exists,
                self.smoke.shutil.which,
                self.smoke.link_route_snapshot,
                self.smoke.cli_json,
            ) = originals

        self.assertTrue(report["ok"])
        self.assertFalse(report["mutated"])
        self.assertEqual(report["mode"], "preflight")


if __name__ == "__main__":
    unittest.main()
