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
                    "grok-4.6",
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
            self.assertEqual(isolated_settings["model"], "grok-4.6")
            self.assertEqual(isolated_settings["effortLevel"], "high")
            self.assertEqual(
                isolated_settings["env"]["CLAUDE_CODE_STOP_HOOK_BLOCK_CAP"], "64"
            )
            self.assertEqual(isolated_settings["advisorModel"], "opus")
            isolated_hooks = isolated_settings["hooks"]
            self.assertIn("PreToolUse", isolated_hooks)
            self.assertIn("PostToolUse", isolated_hooks)
            self.assertIn("SubagentStop", isolated_hooks)
            pre = json.dumps(isolated_hooks["PreToolUse"])
            self.assertIn("claudex-tool-policy", pre)
            # Plain shared settings must not gain mechanical tool limits.
            self.assertNotIn("PreToolUse", shared.get("hooks", {}))

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
            self.assertNotIn(
                "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
                isolated_settings.get("env", {}),
            )
            self.assertEqual(
                isolated_settings["env"]["CLAUDE_CODE_STOP_HOOK_BLOCK_CAP"], "64"
            )

    def test_writes_and_clears_unknown_model_context_window(self) -> None:
        with tempfile.TemporaryDirectory(prefix="claudex-prepare-window-") as temporary:
            home = Path(temporary)
            user_claude = home / ".claude"
            isolated = home / "isolated"
            user_claude.mkdir()
            (user_claude / "settings.json").write_text(
                json.dumps(
                    {
                        "model": "opus",
                        "effortLevel": "high",
                        "env": {"KEEP": "1"},
                    }
                ),
                encoding="utf-8",
            )
            result = subprocess.run(
                [
                    "python3",
                    str(PREPARE),
                    str(user_claude),
                    str(isolated),
                    "gpt-5.6-terra",
                    "high",
                    "110000",
                ],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            isolated_settings = json.loads(
                (isolated / "settings.json").read_text(encoding="utf-8")
            )
            self.assertEqual(isolated_settings["model"], "gpt-5.6-terra")
            self.assertEqual(
                isolated_settings["env"]["CLAUDE_CODE_MAX_CONTEXT_TOKENS"], "110000"
            )
            self.assertEqual(isolated_settings["env"]["KEEP"], "1")
            self.assertEqual(
                isolated_settings["env"]["CLAUDE_CODE_STOP_HOOK_BLOCK_CAP"], "64"
            )

            cleared = subprocess.run(
                [
                    "python3",
                    str(PREPARE),
                    str(user_claude),
                    str(isolated),
                    "opus",
                    "high",
                ],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(cleared.returncode, 0, cleared.stderr)
            isolated_settings = json.loads(
                (isolated / "settings.json").read_text(encoding="utf-8")
            )
            self.assertNotIn(
                "CLAUDE_CODE_MAX_CONTEXT_TOKENS", isolated_settings.get("env", {})
            )
            self.assertEqual(isolated_settings["env"]["KEEP"], "1")
            self.assertEqual(
                isolated_settings["env"]["CLAUDE_CODE_STOP_HOOK_BLOCK_CAP"], "64"
            )


if __name__ == "__main__":
    unittest.main()
