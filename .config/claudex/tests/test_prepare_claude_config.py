#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from types import ModuleType


ROOT = Path(__file__).resolve().parents[1]
PREPARE = ROOT / "prepare-claude-config.py"


def load_prepare_module() -> ModuleType:
    spec = importlib.util.spec_from_file_location("prepare_claude_config", PREPARE)
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load prepare-claude-config.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


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
            self.assertEqual(isolated_settings["modelOverrides"]["grok-4.6"], "grok-4.6")
            self.assertNotIn("grok-4.5", isolated_settings["modelOverrides"])
            self.assertEqual(
                isolated_settings["env"]["CLAUDE_CODE_STOP_HOOK_BLOCK_CAP"], "64"
            )
            self.assertEqual(isolated_settings["advisorModel"], "opus")
            isolated_hooks = isolated_settings["hooks"]
            self.assertIn("PreToolUse", isolated_hooks)
            self.assertIn("PostToolUse", isolated_hooks)
            self.assertIn("SubagentStop", isolated_hooks)
            self.assertIn("SessionEnd", isolated_hooks)
            pre = json.dumps(isolated_hooks["PreToolUse"])
            self.assertIn("claudex-tool-policy", pre)
            # Plain shared settings must not gain mechanical tool limits.
            self.assertNotIn("PreToolUse", shared.get("hooks", {}))

            agents_link = isolated / "agents"
            self.assertTrue(agents_link.is_symlink())
            self.assertEqual(agents_link.resolve(), (user_claude / "agents").resolve())
            self.assertFalse((isolated / "settings.local.json").exists())
            denylist = home / ".config/claudex/disabled-subagent-models.json"
            self.assertTrue(denylist.is_file())
            self.assertEqual(
                json.loads(denylist.read_text(encoding="utf-8")),
                {"version": 1, "disabledModels": []},
            )

    def test_does_not_overwrite_existing_denylist(self) -> None:
        with tempfile.TemporaryDirectory(prefix="claudex-prepare-denylist-") as temporary:
            home = Path(temporary)
            user_claude = home / ".claude"
            isolated = home / ".config/claudex/claude-config"
            user_claude.mkdir(parents=True)
            (user_claude / "settings.json").write_text(
                '{"model":"opus","effortLevel":"high"}',
                encoding="utf-8",
            )
            denylist = home / ".config/claudex/disabled-subagent-models.json"
            denylist.parent.mkdir(parents=True)
            denylist.write_text(
                '{"version":1,"disabledModels":["grok-4.6"]}\n',
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
            self.assertEqual(
                json.loads(denylist.read_text(encoding="utf-8")),
                {"version": 1, "disabledModels": ["grok-4.6"]},
            )

    def test_seeds_blank_denylist_beside_isolated_config(self) -> None:
        with tempfile.TemporaryDirectory(prefix="claudex-prepare-blank-denylist-") as temporary:
            home = Path(temporary)
            user_claude = home / ".claude"
            isolated = home / ".config/claudex/claude-config"
            user_claude.mkdir(parents=True)
            (user_claude / "settings.json").write_text(
                '{"model":"opus","effortLevel":"high"}',
                encoding="utf-8",
            )
            denylist = home / ".config/claudex/disabled-subagent-models.json"
            denylist.parent.mkdir(parents=True)
            denylist.write_text(" \n", encoding="utf-8")
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
            self.assertEqual(
                json.loads(denylist.read_text(encoding="utf-8")),
                {"version": 1, "disabledModels": []},
            )

    def test_rejects_denylist_path_that_is_a_directory(self) -> None:
        with tempfile.TemporaryDirectory(prefix="claudex-prepare-dir-denylist-") as temporary:
            path = Path(temporary) / "disabled-subagent-models.json"
            path.mkdir()
            module = load_prepare_module()
            with self.assertRaises(ValueError) as raised:
                module.ensure_disabled_subagent_models(path)
            self.assertIn(str(path), str(raised.exception))

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
            self.assertFalse((home / "disabled-subagent-models.json").exists())

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

    def test_maps_grok_ids_in_isolated_model_overrides(self) -> None:
        with tempfile.TemporaryDirectory(prefix="claudex-prepare-overrides-") as temporary:
            home = Path(temporary)
            user_claude = home / ".claude"
            isolated = home / "isolated"
            user_claude.mkdir()
            (user_claude / "settings.json").write_text(
                json.dumps(
                    {
                        "model": "opus",
                        "effortLevel": "high",
                        "modelOverrides": {
                            "claude-opus-4-6": "arn:aws:bedrock:example",
                            "grok-4.5": "grok-4.5",
                        },
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
                    "cursor/gpt-5.6-terra",
                    "max",
                    "800000",
                ],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            isolated_settings = json.loads(
                (isolated / "settings.json").read_text(encoding="utf-8")
            )
            overrides = isolated_settings["modelOverrides"]
            self.assertEqual(overrides["grok-4.6"], "grok-4.6")
            self.assertEqual(
                overrides["cursor/gpt-5.6-luna"], "cursor/gpt-5.6-luna"
            )
            self.assertEqual(overrides["cursor/gpt-5.6-sol"], "cursor/gpt-5.6-sol")
            self.assertEqual(
                overrides["cursor/gpt-5.6-terra"], "cursor/gpt-5.6-terra"
            )
            self.assertNotIn("grok-4.5", overrides)
            self.assertEqual(overrides["claude-opus-4-6"], "arn:aws:bedrock:example")
            self.assertEqual(isolated_settings["model"], "cursor/gpt-5.6-terra")
            self.assertEqual(
                isolated_settings["env"]["CLAUDE_CODE_MAX_CONTEXT_TOKENS"], "800000"
            )
            shared = json.loads((user_claude / "settings.json").read_text(encoding="utf-8"))
            self.assertNotIn("grok-4.6", shared.get("modelOverrides", {}))
            self.assertNotIn("cursor/gpt-5.6-terra", shared.get("modelOverrides", {}))


if __name__ == "__main__":
    unittest.main()
