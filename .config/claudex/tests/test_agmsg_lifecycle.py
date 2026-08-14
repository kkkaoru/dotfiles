#!/usr/bin/env python3
"""Regression tests for agmsg delivery boundaries around claudex children."""

from __future__ import annotations

import json
import hashlib
import os
from pathlib import Path
import stat
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[3]
GUARD_INSTALLER = ROOT / "scripts/ensure-agmsg-claudex-guard.sh"
CLAUDEX_FUNCTION = ROOT / ".config/fish/functions/claudex.fish"
COMMAND_DOC = ROOT / ".claude/commands/agmsg.md"
SYMLINK_INSTALLER = ROOT / "create-symlinks.sh"
PROJECT = ROOT

CHILD_MARKER = "# claudex: provider-backed children do not own agmsg watchers."
PARENT_MARKER = "# claudex: automatic agmsg Monitor is opt-in for the interactive parent."
INBOX_PARENT_MARKER = "# claudex: agmsg turn delivery is opt-in for the interactive parent."
WATCH_CHILD_MARKER = "# claudex: provider/noninteractive child watchers are disabled."
RESUME_MARKER = "# claudex: resumed claudex sessions do not run agmsg watchers."
CLAIM_MARKER = "# claudex: serialize same-session watcher claims."
CLAIM_VERSION = "# claudex: watcher claim schema v2."
EXPLICIT_MARKER = "CLAUDEX_AGMSG_EXPLICIT=1"


def write_scripts(scripts: Path, *, malformed: bool = False) -> None:
    """Create four representative agmsg scripts without touching ~/.agents."""

    child_guard = f'''{CHILD_MARKER}
if [ "${{CLAUDEX_NONINTERACTIVE_CHILD:-}}" = 1 ] \\
  || [ "${{CLAUDEX_PROVIDER_ACP:-}}" = 1 ] \\
  || [ "${{CLAUDEX_GROK_ACP:-}}" = 1 ]; then
  exit 0
fi
'''
    parent_guard = f'''{PARENT_MARKER}
if [ "${{CLAUDEX_ACTIVE:-}}" = 1 ] && [ "${{CLAUDEX_AGMSG_AUTO_MONITOR:-}}" != 1 ]; then
  exit 0
fi
'''
    inbox_parent_guard = f'''{INBOX_PARENT_MARKER}
if [ "${{CLAUDEX_ACTIVE:-}}" = 1 ] && [ "${{CLAUDEX_AGMSG_AUTO_MONITOR:-}}" != 1 ]; then
  exit 0
fi
'''
    watch_parent_guard = f'''{PARENT_MARKER}
if [ "${{CLAUDEX_ACTIVE:-}}" = 1 ] \\
  && [ "${{CLAUDEX_AGMSG_AUTO_MONITOR:-}}" != 1 ] \\
  && [ "${{CLAUDEX_AGMSG_EXPLICIT:-}}" != 1 ]; then
  exit 0
fi
'''
    watch_child_guard = f'''{WATCH_CHILD_MARKER}
if [ "${{CLAUDEX_NONINTERACTIVE_CHILD:-}}" = 1 ] \\
  || [ "${{CLAUDEX_PROVIDER_ACP:-}}" = 1 ] \\
  || [ "${{CLAUDEX_GROK_ACP:-}}" = 1 ]; then
  exit 0
fi
'''
    watch_resume_guard = f'''{RESUME_MARKER}
if [ "${{CLAUDEX_AGMSG_AUTO_MONITOR:-}}" != 1 ] \\
  && [ "${{CLAUDEX_AGMSG_EXPLICIT:-}}" != 1 ] \\
  && [ -n "${{1:-}}" ]; then
  if false; then
    exit 0
  fi
fi
'''
    if malformed:
        malformed_child = f'''{CHILD_MARKER}
if [ "${{CLAUDEX_NONINTERACTIVE_CHILD:-}}" = 1 \\
  || [ "${{CLAUDEX_PROVIDER_ACP:-}}" = 1 \\
  || [ "${{CLAUDEX_GROK_ACP:-}}" = 1 ]; then
  exit 0
fi
'''.replace('] \\\n  || [', ' \\\n  || [', 2)
        malformed_parent = f'''{PARENT_MARKER}
if [ "${{CLAUDEX_ACTIVE:-}}" = 1 ] && [ "${{CLAUDEX_AGMSG_AUTO_MONITOR:-}}" != 1 ]; then
  exit 0
fi
'''
        malformed_inbox_parent = f'''{INBOX_PARENT_MARKER}
if [ "${{CLAUDEX_ACTIVE:-}}" = 1 ] && [ "${{CLAUDEX_AGMSG_AUTO_MONITOR:-}}" != 1 ]; then
  exit 0
fi
'''
        malformed_watch_parent = f'''{PARENT_MARKER}
if [ "${{CLAUDEX_ACTIVE:-}}" = 1 \\
  && [ "${{CLAUDEX_AGMSG_AUTO_MONITOR:-}}" != 1 \\
  && [ "${{CLAUDEX_AGMSG_EXPLICIT:-}}" != 1 ]; then
  exit 0
fi
'''.replace('] \\\n  && [', ' \\\n  && [', 2)
        malformed_watch_child = malformed_child.replace(CHILD_MARKER, WATCH_CHILD_MARKER, 1)
        malformed_watch_resume = watch_resume_guard
    else:
        malformed_child = malformed_parent = malformed_inbox_parent = ""
        malformed_watch_parent = malformed_watch_child = malformed_watch_resume = ""

    session_start = "#!/usr/bin/env bash\n" + malformed_child + malformed_parent
    session_start += "set -euo pipefail\nprintf '%s\\n' session-start-body\n"
    session_end = "#!/usr/bin/env bash\n" + malformed_child
    session_end += "set -euo pipefail\nprintf '%s\\n' session-end-body\n"
    check_inbox = "#!/usr/bin/env bash\n" + malformed_child + malformed_inbox_parent
    check_inbox += "set -euo pipefail\nprintf '%s\\n' check-inbox-body\n"
    watch = "#!/usr/bin/env bash\n"
    watch += malformed_watch_parent + malformed_watch_child + malformed_watch_resume
    watch += (
        "set -u\n"
        'RUN_DIR="${AGMSG_TEST_RUN_DIR:?}"\n'
        'SESSION_ID="${1:-fixture-session}"\n'
        'PIDFILE="$RUN_DIR/watch.$SESSION_ID.pid"\n'
        "echo $$ > \"$PIDFILE\"\n"
        "printf '%s\\n' watch-body\n"
    )
    for name, text in (
        ("session-start.sh", session_start),
        ("session-end.sh", session_end),
        ("check-inbox.sh", check_inbox),
        ("watch.sh", watch),
    ):
        path = scripts / name
        path.write_text(text, encoding="utf-8")
        path.chmod(path.stat().st_mode | stat.S_IXUSR)


class AgmsgLifecycleTests(unittest.TestCase):
    def run_installer(self, scripts: Path) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [str(GUARD_INSTALLER)],
            cwd=PROJECT,
            env={**os.environ, "AGMSG_SCRIPTS_DIR": str(scripts)},
            capture_output=True,
            text=True,
            check=False,
        )

    def assert_bash_syntax(self, scripts: Path) -> None:
        for path in sorted(scripts.glob("*.sh")):
            result = subprocess.run(
                ["bash", "-n", str(path)],
                cwd=PROJECT,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, f"{path}: {result.stderr}")

    def script_hashes(self, scripts: Path) -> dict[str, str]:
        return {
            name: hashlib.sha256((scripts / name).read_bytes()).hexdigest()
            for name in ("session-start.sh", "session-end.sh", "check-inbox.sh", "watch.sh")
        }

    def run_script(
        self,
        scripts: Path,
        name: str,
        environment: dict[str, str] | None = None,
        *,
        input_text: str = "",
        timeout: float = 1.0,
    ) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory(prefix="claudex-agmsg-run-") as temporary:
            env = {
                **os.environ,
                "AGMSG_TEST_RUN_DIR": str(Path(temporary) / "run"),
                **(environment or {}),
            }
            Path(env["AGMSG_TEST_RUN_DIR"]).mkdir()
            return subprocess.run(
                ["bash", str(scripts / name), "fixture-session", str(PROJECT), "claude-code"],
                cwd=PROJECT,
                env=env,
                input=input_text,
                capture_output=True,
                text=True,
                check=False,
                timeout=timeout,
            )

    def prepare_launcher_home(self, home: Path, *, failing_installer: bool = False) -> Path:
        scripts = home / "agmsg-scripts"
        scripts.mkdir()
        write_scripts(scripts)
        (home / ".local/bin").mkdir(parents=True)
        (home / ".config/claudex").mkdir(parents=True)
        (home / ".claude").mkdir()
        (home / ".config/claudex/providers.json").write_text(
            json.dumps(
                {
                    "version": 1,
                    "mainProviders": ["provider"],
                    "providers": [
                        {
                            "id": "provider",
                            "agent": "worker",
                            "defaultModel": "provider-model",
                            "effort": "high",
                            "backend": "codex-app-server",
                        }
                    ],
                    "fallback": {"agent": "fallback", "model": "sonnet", "effort": "high"},
                }
            ),
            encoding="utf-8",
        )
        (home / ".claude/settings.json").write_text(
            '{"model":"sonnet[1m]","effortLevel":"high"}',
            encoding="utf-8",
        )
        installer = home / ".local/bin/ensure-agmsg-claudex-guard"
        if failing_installer:
            installer.write_text(
                "#!/bin/sh\nprintf 'simulated guard failure\\n' >&2\nexit 17\n",
                encoding="utf-8",
            )
            installer.chmod(installer.stat().st_mode | stat.S_IXUSR)
        else:
            installer.symlink_to(GUARD_INSTALLER)
        adapter = home / ".local/bin/claudex-agent-adapter"
        adapter.write_text("#!/bin/sh\nprintf 'launcher-ran\\n'\n", encoding="utf-8")
        adapter.chmod(adapter.stat().st_mode | stat.S_IXUSR)
        return scripts

    def run_fish_launcher(self, home: Path, scripts: Path) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                "fish",
                "--no-config",
                "-c",
                f"source '{CLAUDEX_FUNCTION}'; claudex launch-reapply",
            ],
            cwd=PROJECT,
            env={
                **os.environ,
                "HOME": str(home),
                "AGMSG_SCRIPTS_DIR": str(scripts),
            },
            capture_output=True,
            text=True,
            check=False,
            timeout=5.0,
        )

    def test_guard_installer_migrates_malformed_blocks_and_repairs_all_four_files(self) -> None:
        with tempfile.TemporaryDirectory(prefix="claudex-agmsg-malformed-") as temporary:
            scripts = Path(temporary)
            write_scripts(scripts, malformed=True)
            result = self.run_installer(scripts)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assert_bash_syntax(scripts)

            start = (scripts / "session-start.sh").read_text(encoding="utf-8")
            end = (scripts / "session-end.sh").read_text(encoding="utf-8")
            inbox = (scripts / "check-inbox.sh").read_text(encoding="utf-8")
            watcher = (scripts / "watch.sh").read_text(encoding="utf-8")
            self.assertEqual(start.count(CHILD_MARKER), 1)
            self.assertEqual(start.count(PARENT_MARKER), 1)
            self.assertEqual(end.count(CHILD_MARKER), 1)
            self.assertEqual(inbox.count(CHILD_MARKER), 1)
            self.assertEqual(inbox.count(INBOX_PARENT_MARKER), 1)
            self.assertEqual(watcher.count(PARENT_MARKER), 1)
            self.assertEqual(watcher.count(WATCH_CHILD_MARKER), 1)
            self.assertEqual(watcher.count(RESUME_MARKER), 1)
            self.assertIn('= 1 ] \\\n  || [', start)
            self.assertNotIn('= 1 \\\n  || [', start)
            self.assertNotIn('= 1 \\\n  || [', watcher)
            self.assertIn(CLAIM_MARKER, watcher)

    def test_guard_installer_pristine_reapply_is_idempotent_and_injects_actual_files(self) -> None:
        with tempfile.TemporaryDirectory(prefix="claudex-agmsg-pristine-") as temporary:
            scripts = Path(temporary)
            write_scripts(scripts)
            modes = {
                name: stat.S_IMODE((scripts / name).stat().st_mode)
                for name in ("session-start.sh", "session-end.sh", "check-inbox.sh", "watch.sh")
            }
            first = self.run_installer(scripts)
            self.assertEqual(first.returncode, 0, first.stderr)
            first_text = {
                name: (scripts / name).read_text(encoding="utf-8")
                for name in ("session-start.sh", "session-end.sh", "check-inbox.sh", "watch.sh")
            }
            self.assertEqual(
                {
                    name: stat.S_IMODE((scripts / name).stat().st_mode)
                    for name in modes
                },
                modes,
            )
            for name, text in first_text.items():
                expected_child_marker = WATCH_CHILD_MARKER if name == "watch.sh" else CHILD_MARKER
                self.assertIn(expected_child_marker, text)
                if name != "session-end.sh":
                    self.assertIn("CLAUDEX_AGMSG", text)
            self.assert_bash_syntax(scripts)

            second = self.run_installer(scripts)
            self.assertEqual(second.returncode, 0, second.stderr)
            second_text = {
                name: (scripts / name).read_text(encoding="utf-8")
                for name in first_text
            }
            self.assertEqual(second_text, first_text)
            for name, text in second_text.items():
                expected_child_marker = WATCH_CHILD_MARKER if name == "watch.sh" else CHILD_MARKER
                self.assertEqual(text.count(expected_child_marker), 1)

    def test_guard_installer_anchor_drift_is_non_mutating(self) -> None:
        with tempfile.TemporaryDirectory(prefix="claudex-agmsg-anchor-drift-") as temporary:
            scripts = Path(temporary)
            write_scripts(scripts)
            watcher = scripts / "watch.sh"
            watcher.write_text(
                watcher.read_text(encoding="utf-8").replace(
                    'PIDFILE="$RUN_DIR/watch.$SESSION_ID.pid"',
                    'PIDFILE="$RUN_DIR/watch.$SESSION_ID.drift.pid"',
                    1,
                ),
                encoding="utf-8",
            )
            before = self.script_hashes(scripts)
            result = self.run_installer(scripts)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(self.script_hashes(scripts), before)

    def test_guard_installer_migrates_stale_malformed_watch_claim(self) -> None:
        with tempfile.TemporaryDirectory(prefix="claudex-agmsg-claim-migration-") as temporary:
            scripts = Path(temporary)
            write_scripts(scripts)
            self.assertEqual(self.run_installer(scripts).returncode, 0)
            watcher = scripts / "watch.sh"
            stale = watcher.read_text(encoding="utf-8")
            stale = stale.replace(CLAIM_VERSION + "\n", "", 1)
            stale = stale.replace("CLAIM_ATTEMPTS=0", "CLAIM_ATTEMPTS=2", 1)
            stale = stale.replace(
                '  echo "agmsg watch: timed out claiming watcher slot for $SESSION_ID" >&2\n'
                '  exit 1\n'
                'fi\n'
                'echo $$ > "$PIDFILE"',
                '  echo "agmsg watch: timed out claiming watcher slot for $SESSION_ID" >&2\n'
                '  exit 1\n'
                'echo $$ > "$PIDFILE"',
                1,
            )
            watcher.write_text(stale, encoding="utf-8")
            invalid = subprocess.run(
                ["bash", "-n", str(watcher)],
                cwd=PROJECT,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertNotEqual(invalid.returncode, 0)

            result = self.run_installer(scripts)
            self.assertEqual(result.returncode, 0, result.stderr)
            migrated = watcher.read_text(encoding="utf-8")
            self.assertEqual(migrated.count(CLAIM_MARKER), 1)
            self.assertEqual(migrated.count(CLAIM_VERSION), 1)
            self.assertIn('CLAIM_ATTEMPTS=0', migrated)
            self.assert_bash_syntax(scripts)

    def test_noninteractive_hooks_exit_without_waiting_for_stdin(self) -> None:
        with tempfile.TemporaryDirectory(prefix="claudex-agmsg-guard-") as temporary:
            scripts = Path(temporary)
            write_scripts(scripts)
            self.assertEqual(self.run_installer(scripts).returncode, 0)
            for name in ("session-start.sh", "session-end.sh", "check-inbox.sh"):
                with self.subTest(script=name):
                    environment = {
                        "CLAUDEX_NONINTERACTIVE_CHILD": "1",
                    }
                    process = subprocess.Popen(
                        ["bash", str(scripts / name), "claude-code", str(PROJECT)],
                        cwd=PROJECT,
                        env={**os.environ, **environment},
                        stdin=subprocess.PIPE,
                        stdout=subprocess.PIPE,
                        stderr=subprocess.PIPE,
                        text=True,
                    )
                    returncode = process.wait(timeout=1.0)
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

    def test_provider_markers_are_guarded_before_monitor_start(self) -> None:
        with tempfile.TemporaryDirectory(prefix="claudex-agmsg-marker-") as temporary:
            scripts = Path(temporary)
            write_scripts(scripts)
            self.assertEqual(self.run_installer(scripts).returncode, 0)
            for marker in (
                "CLAUDEX_NONINTERACTIVE_CHILD",
                "CLAUDEX_PROVIDER_ACP",
                "CLAUDEX_GROK_ACP",
            ):
                with self.subTest(marker=marker):
                    process = self.run_script(
                        scripts,
                        "session-start.sh",
                        {marker: "1"},
                    )
                    self.assertEqual(process.returncode, 0, process.stderr)
                    self.assertEqual(process.stdout, "")

    def test_parent_explicit_watch_passes_but_provider_child_explicit_watch_exits(self) -> None:
        with tempfile.TemporaryDirectory(prefix="claudex-agmsg-explicit-") as temporary:
            scripts = Path(temporary)
            write_scripts(scripts)
            self.assertEqual(self.run_installer(scripts).returncode, 0)

            parent = self.run_script(
                scripts,
                "watch.sh",
                {
                    "CLAUDEX_ACTIVE": "1",
                    "CLAUDEX_AGMSG_EXPLICIT": "1",
                },
            )
            self.assertEqual(parent.returncode, 0, parent.stderr)
            self.assertEqual(parent.stdout, "watch-body\n")

            for marker in (
                "CLAUDEX_NONINTERACTIVE_CHILD",
                "CLAUDEX_PROVIDER_ACP",
                "CLAUDEX_GROK_ACP",
            ):
                with self.subTest(marker=marker):
                    child = self.run_script(
                        scripts,
                        "watch.sh",
                        {
                            "CLAUDEX_ACTIVE": "1",
                            "CLAUDEX_AGMSG_EXPLICIT": "1",
                            marker: "1",
                        },
                    )
                    self.assertEqual(child.returncode, 0, child.stderr)
                    self.assertEqual(child.stdout, "")

    def test_automatic_default_suppression_and_auto_monitor_opt_in(self) -> None:
        with tempfile.TemporaryDirectory(prefix="claudex-agmsg-default-") as temporary:
            scripts = Path(temporary)
            write_scripts(scripts)
            self.assertEqual(self.run_installer(scripts).returncode, 0)

            suppressed = self.run_script(
                scripts,
                "watch.sh",
                {"CLAUDEX_ACTIVE": "1"},
            )
            self.assertEqual(suppressed.returncode, 0, suppressed.stderr)
            self.assertEqual(suppressed.stdout, "")

            permitted = self.run_script(
                scripts,
                "watch.sh",
                {
                    "CLAUDEX_ACTIVE": "1",
                    "CLAUDEX_AGMSG_AUTO_MONITOR": "1",
                },
            )
            self.assertEqual(permitted.returncode, 0, permitted.stderr)
            self.assertEqual(permitted.stdout, "watch-body\n")

            start_permitted = self.run_script(
                scripts,
                "session-start.sh",
                {"CLAUDEX_ACTIVE": "1", "CLAUDEX_AGMSG_AUTO_MONITOR": "1"},
            )
            self.assertEqual(start_permitted.returncode, 0, start_permitted.stderr)
            self.assertEqual(start_permitted.stdout, "session-start-body\n")

            inbox_permitted = self.run_script(
                scripts,
                "check-inbox.sh",
                {"CLAUDEX_ACTIVE": "1", "CLAUDEX_AGMSG_AUTO_MONITOR": "1"},
            )
            self.assertEqual(inbox_permitted.returncode, 0, inbox_permitted.stderr)
            self.assertEqual(inbox_permitted.stdout, "check-inbox-body\n")

            inbox = self.run_script(
                scripts,
                "check-inbox.sh",
                {"CLAUDEX_ACTIVE": "1", "CLAUDEX_AGMSG_EXPLICIT": "1"},
            )
            self.assertEqual(inbox.returncode, 0, inbox.stderr)
            self.assertEqual(inbox.stdout, "")

            start = self.run_script(
                scripts,
                "session-start.sh",
                {"CLAUDEX_ACTIVE": "1", "CLAUDEX_AGMSG_EXPLICIT": "1"},
            )
            self.assertEqual(start.returncode, 0, start.stderr)
            self.assertEqual(start.stdout, "")

    def test_command_docs_and_launch_reapply_restore_actual_hooks(self) -> None:
        docs = COMMAND_DOC.read_text(encoding="utf-8")
        watch_commands = [line for line in docs.splitlines() if "watch.sh" in line and "command:" in line]
        self.assertGreaterEqual(len(watch_commands), 3)
        self.assertTrue(all(EXPLICIT_MARKER in line for line in watch_commands))
        self.assertIn("provider/noninteractive children still", docs)
        symlink_script = SYMLINK_INSTALLER.read_text(encoding="utf-8")
        self.assertIn("ensure-agmsg-claudex-guard.sh", symlink_script)
        function = CLAUDEX_FUNCTION.read_text(encoding="utf-8")
        self.assertIn("command \"$agmsg_guard_installer\"", function)
        self.assertIn("agmsg guard refresh failed", function)

        with tempfile.TemporaryDirectory(prefix="claudex-agmsg-launch-") as temporary:
            home = Path(temporary)
            scripts = self.prepare_launcher_home(home)
            result = self.run_fish_launcher(home, scripts)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("launcher-ran", result.stdout)
            for name in ("session-start.sh", "session-end.sh", "check-inbox.sh", "watch.sh"):
                text = (scripts / name).read_text(encoding="utf-8")
                marker = WATCH_CHILD_MARKER if name == "watch.sh" else CHILD_MARKER
                self.assertIn(marker, text)

        with tempfile.TemporaryDirectory(prefix="claudex-agmsg-launch-failure-") as temporary:
            home = Path(temporary)
            scripts = self.prepare_launcher_home(home, failing_installer=True)
            result = self.run_fish_launcher(home, scripts)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("launcher-ran", result.stdout)
            self.assertIn("simulated guard failure", result.stderr)
            self.assertIn("agmsg guard refresh failed (exit 17)", result.stderr)

    def test_parent_inbox_boundary_does_not_inject_agmsg_into_claudex_turn(self) -> None:
        with tempfile.TemporaryDirectory(prefix="claudex-agmsg-inbox-") as temporary:
            scripts = Path(temporary)
            write_scripts(scripts)
            self.assertEqual(self.run_installer(scripts).returncode, 0)
            process = self.run_script(
                scripts,
                "check-inbox.sh",
                {"CLAUDEX_ACTIVE": "1", "CLAUDEX_AGMSG_AUTO_MONITOR": "0"},
                input_text='{"session_id":"fixture-session"}',
            )
            self.assertEqual(process.returncode, 0, process.stderr)
            self.assertEqual(process.stdout, "")
            self.assertEqual(process.stderr, "")


if __name__ == "__main__":
    unittest.main()
