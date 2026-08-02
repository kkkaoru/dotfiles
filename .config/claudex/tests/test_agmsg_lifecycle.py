#!/usr/bin/env python3
"""Regression tests for agmsg delivery boundaries around claudex children."""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


AGMSG_SCRIPTS = Path.home() / ".agents/skills/agmsg/scripts"
PROJECT = Path.cwd()


class AgmsgLifecycleTests(unittest.TestCase):
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
