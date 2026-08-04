#!/usr/bin/env python3

from __future__ import annotations

import json
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PREPARE = ROOT / "prepare-claude-config.py"


class PrepareClaudeConfigTests(unittest.TestCase):
    def test_isolates_model_and_sanitizes_shared_discovery_ids(self) -> None:
        with tempfile.TemporaryDirectory(prefix="claudex-prepare-config-") as temporary:
            home = Path(temporary)
            user_claude = home / ".claude"
            isolated = home / ".config/claudex/claude-config"
            user_claude.mkdir(parents=True)
            (user_claude / "agents").mkdir()
            (user_claude / "settings.json").write_text(
                json.dumps(
                    {
                        "model": "claude-claudex-fugu",
                        "effortLevel": "max",
                        "advisorModel": "opus",
                    }
                ),
                encoding="utf-8",
            )
            (user_claude / "settings.local.json").write_text(
                '{"model":"claude-claudex-auto"}',
                encoding="utf-8",
            )

            result = subprocess.run(
                [
                    "python3",
                    str(PREPARE),
                    str(user_claude),
                    str(isolated),
                    "grok-4.5",
                    "high",
                ],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.strip(), str(isolated.resolve()))

            shared = json.loads((user_claude / "settings.json").read_text(encoding="utf-8"))
            self.assertEqual(shared["model"], "sonnet[1m]")
            self.assertEqual(shared["effortLevel"], "max")
            self.assertEqual(shared["advisorModel"], "opus")

            isolated_settings = json.loads(
                (isolated / "settings.json").read_text(encoding="utf-8")
            )
            self.assertEqual(isolated_settings["model"], "grok-4.5")
            self.assertEqual(isolated_settings["effortLevel"], "high")
            self.assertEqual(isolated_settings["advisorModel"], "opus")

            agents_link = isolated / "agents"
            self.assertTrue(agents_link.is_symlink())
            self.assertEqual(agents_link.resolve(), (user_claude / "agents").resolve())
            self.assertFalse((isolated / "settings.local.json").exists())

    def test_leaves_native_shared_model_untouched(self) -> None:
        with tempfile.TemporaryDirectory(prefix="claudex-prepare-native-") as temporary:
            home = Path(temporary)
            user_claude = home / ".claude"
            isolated = home / "isolated"
            user_claude.mkdir()
            (user_claude / "settings.json").write_text(
                '{"model":"opus","effortLevel":"high"}',
                encoding="utf-8",
            )
            result = subprocess.run(
                [
                    "python3",
                    str(PREPARE),
                    str(user_claude),
                    str(isolated),
                    "gpt-5.6-luna",
                    "max",
                ],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            shared = json.loads((user_claude / "settings.json").read_text(encoding="utf-8"))
            self.assertEqual(shared["model"], "opus")
            isolated_settings = json.loads(
                (isolated / "settings.json").read_text(encoding="utf-8")
            )
            self.assertEqual(isolated_settings["model"], "gpt-5.6-luna")
            self.assertEqual(isolated_settings["effortLevel"], "max")


if __name__ == "__main__":
    unittest.main()
