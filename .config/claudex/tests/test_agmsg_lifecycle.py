#!/usr/bin/env python3
"""Regression tests for agmsg delivery boundaries around claudex children."""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


AGMSG_SCRIPTS = Path.home() / ".agents/skills/agmsg/scripts"
ROOT = Path(__file__).resolve().parents[3]
GUARD_INSTALLER = ROOT / "scripts/ensure-agmsg-claudex-guard.sh"
PROJECT = Path.cwd()


class AgmsgLifecycleTests(unittest.TestCase):
    def test_guard_installer_is_idempotent_for_installed_hook_shapes(self) -> None:
        with tempfile.TemporaryDirectory(prefix="claudex-agmsg-installer-") as temporary:
            scripts = Path(temporary)
            for name in ("session-start.sh", "session-end.sh", "check-inbox.sh"):
                (scripts / name).write_text(
                    "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' body\n",
                    encoding="utf-8",
                )
            environment = {**os.environ, "AGMSG_SCRIPTS_DIR": str(scripts)}
            first = subprocess.run([str(GUARD_INSTALLER)], env=environment, check=False)
            self.assertEqual(first.returncode, 0)
            first_text = (scripts / "session-start.sh").read_text(encoding="utf-8")
            self.assertEqual(first_text.count("provider-backed children do not own agmsg watchers"), 1)
            second = subprocess.run([str(GUARD_INSTALLER)], env=environment, check=False)
            self.assertEqual(second.returncode, 0)
            self.assertEqual((scripts / "session-start.sh").read_text(encoding="utf-8"), first_text)

    def test_noninteractive_hooks_exit_without_waiting_for_stdin(self) -> None:
        scripts = (
            "session-start.sh",
            "session-end.sh",
            "check-inbox.sh",
        )
        with tempfile.TemporaryDirectory(prefix="claudex-agmsg-guard-") as temporary:
            environment = {
                **os.environ,
                "AGMSG_STORAGE_PATH": str(Path(temporary) / "store"),
                "CLAUDEX_NONINTERACTIVE_CHILD": "1",
            }
            for name in scripts:
                with self.subTest(script=name):
                    process = subprocess.Popen(
                        [
                            "bash",
                            str(AGMSG_SCRIPTS / name),
                            "claude-code",
                            str(PROJECT),
                        ],
                        cwd=PROJECT,
                        env=environment,
                        stdin=subprocess.PIPE,
                        stdout=subprocess.PIPE,
                        stderr=subprocess.PIPE,
                        text=True,
                    )
                    try:
                        returncode = process.wait(timeout=1.0)
                    finally:
                        if process.stdin is not None:
                            process.stdin.close()
                    stdout = process.stdout.read() if process.stdout is not None else ""
                    stderr = process.stderr.read() if process.stderr is not None else ""
                    if process.stdout is not None:
                        process.stdout.close()
                    if process.stderr is not None:
                        process.stderr.close()
                    self.assertEqual(returncode, 0, stderr)
                    self.assertEqual(stdout, "")

    def test_provider_markers_are_all_guarded_before_monitor_start(self) -> None:
        for marker in (
            "CLAUDEX_NONINTERACTIVE_CHILD",
            "CLAUDEX_PROVIDER_ACP",
            "CLAUDEX_GROK_ACP",
        ):
            with self.subTest(marker=marker):
                with tempfile.TemporaryDirectory(prefix="claudex-agmsg-marker-") as temporary:
                    environment = {
                        **os.environ,
                        "AGMSG_STORAGE_PATH": str(Path(temporary) / "store"),
                        marker: "1",
                    }
                    process = subprocess.run(
                        [
                            "bash",
                            str(AGMSG_SCRIPTS / "session-start.sh"),
                            "claude-code",
                            str(PROJECT),
                        ],
                        cwd=PROJECT,
                        env=environment,
                        input="",
                        capture_output=True,
                        text=True,
                        check=False,
                        timeout=1.0,
                    )
                    self.assertEqual(process.returncode, 0, process.stderr)
                    self.assertNotIn("Monitor", process.stdout)


if __name__ == "__main__":
    unittest.main()
