#!/usr/bin/env python3
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import tempfile
import textwrap
import unittest


SCRIPT_PATH = Path(__file__).with_name("clash-tui-system-proxy-gnome-acceptance.sh")


FAKE_SMOKE = r"""
import json
import os
import sys
from pathlib import Path

args = sys.argv[1:]
log_path = Path(os.environ["FAKE_GNOME_SMOKE_LOG"])
output = Path(args[args.index("--output") + 1]) if "--output" in args else None

if "--preflight" in args:
    mode = "preflight"
    code = 0
    report = {
        "ok": True,
        "mode": "preflight",
        "mutated": False,
        "urlLeak": False,
    }
elif "--verify-report" in args:
    mode = "verify-report"
    source = json.loads(Path(args[args.index("--verify-report") + 1]).read_text(encoding="utf-8"))
    ok = source.get("ok") is True and source.get("mode") == "smoke"
    code = 0 if ok else 1
    report = {
        "ok": ok,
        "mode": "verify-report",
        "mutated": False,
        "urlLeak": False,
        "sourceMode": source.get("mode"),
    }
elif "--verify-acceptance-dir" in args:
    mode = "verify-acceptance"
    source_dir = Path(args[args.index("--verify-acceptance-dir") + 1])
    required = [
        source_dir / "gnome-preflight.json",
        source_dir / "gnome-smoke.json",
        source_dir / "gnome-smoke-verified.json",
    ]
    ok = all(path.exists() for path in required)
    code = 0 if ok else 1
    report = {
        "ok": ok,
        "mode": "verify-acceptance",
        "mutated": False,
        "urlLeak": False,
    }
else:
    mode = "smoke"
    code = 0
    report = {
        "ok": True,
        "mode": "smoke",
        "confirmed": "--yes" in args,
        "mutated": True,
        "urlLeak": False,
    }

with log_path.open("a", encoding="utf-8") as handle:
    handle.write(json.dumps({
        "mode": mode,
        "args": args,
        "confirmEnv": os.environ.get("CLASH_TUI_SYSTEM_PROXY_SMOKE"),
    }, sort_keys=True) + "\n")

text = json.dumps(report, sort_keys=True)
if output is not None:
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(text + "\n", encoding="utf-8")
print(text)
raise SystemExit(code)
"""


class GnomeAcceptanceScriptTests(unittest.TestCase):
    def make_fake_smoke(self, temp_dir: Path) -> tuple[Path, Path]:
        fake = temp_dir / "fake-smoke.py"
        log = temp_dir / "fake-smoke.log"
        fake.write_text(textwrap.dedent(FAKE_SMOKE).strip() + "\n", encoding="utf-8")
        return fake, log

    def run_acceptance(self, temp_dir: Path, *extra_args: str):
        fake, log = self.make_fake_smoke(temp_dir)
        output_dir = temp_dir / "reports"
        env = os.environ.copy()
        env["FAKE_GNOME_SMOKE_LOG"] = str(log)
        result = subprocess.run(
            [
                "bash",
                str(SCRIPT_PATH),
                "--smoke-script",
                str(fake),
                "--bin",
                "fake-clash-tui",
                "--output-dir",
                str(output_dir),
                *extra_args,
            ],
            text=True,
            capture_output=True,
            env=env,
            check=False,
        )
        calls = [
            json.loads(line)
            for line in log.read_text(encoding="utf-8").splitlines()
        ] if log.exists() else []
        return result, output_dir, calls

    def test_acceptance_stops_after_preflight_without_yes(self):
        with tempfile.TemporaryDirectory() as raw:
            result, output_dir, calls = self.run_acceptance(Path(raw))

            self.assertEqual(result.returncode, 2)
            self.assertTrue((output_dir / "gnome-preflight.json").exists())
            self.assertFalse((output_dir / "gnome-smoke.json").exists())
            self.assertFalse((output_dir / "gnome-smoke-verified.json").exists())
            self.assertFalse((output_dir / "gnome-acceptance-verified.json").exists())
            self.assertEqual([call["mode"] for call in calls], ["preflight"])
            self.assertIn("--yes", result.stderr)

    def test_acceptance_runs_smoke_and_verify_with_yes(self):
        with tempfile.TemporaryDirectory() as raw:
            result, output_dir, calls = self.run_acceptance(Path(raw), "--yes")

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue((output_dir / "gnome-preflight.json").exists())
            self.assertTrue((output_dir / "gnome-smoke.json").exists())
            self.assertTrue((output_dir / "gnome-smoke-verified.json").exists())
            self.assertTrue((output_dir / "gnome-acceptance-verified.json").exists())
            self.assertEqual(
                [call["mode"] for call in calls],
                ["preflight", "smoke", "verify-report", "verify-acceptance"],
            )
            self.assertEqual(calls[1]["confirmEnv"], "1")
            self.assertIn("--yes", calls[1]["args"])
            verified = json.loads((output_dir / "gnome-smoke-verified.json").read_text(encoding="utf-8"))
            self.assertTrue(verified["ok"])
            self.assertFalse(verified["mutated"])
            acceptance = json.loads((output_dir / "gnome-acceptance-verified.json").read_text(encoding="utf-8"))
            self.assertTrue(acceptance["ok"])
            self.assertFalse(acceptance["mutated"])


if __name__ == "__main__":
    unittest.main()
