#!/usr/bin/env python3

from __future__ import annotations

import json
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
RESOLVE = ROOT / "resolve-context-tokens.py"
PROVIDERS = ROOT / "providers.json"


class ResolveContextTokensTests(unittest.TestCase):
    def test_terra_and_luna_use_codex_window(self) -> None:
        for model in ["gpt-5.6-terra", "claude-claudex-gpt-5.6-terra", "gpt-5.6-luna"]:
            self.assertEqual(self.resolve(str(PROVIDERS), model), "110000", model)

    def test_fugu_uses_one_million_window(self) -> None:
        self.assertEqual(self.resolve(str(PROVIDERS), "fugu"), "1000000")

    def test_native_claude_and_grok_have_no_override(self) -> None:
        for model in ["opus", "sonnet[1m]", "claude-sonnet-5", "grok-4.5"]:
            self.assertEqual(self.resolve(str(PROVIDERS), model), "", model)

    def test_longest_prefix_wins_when_windows_differ(self) -> None:
        with tempfile.TemporaryDirectory(prefix="claudex-context-tokens-") as temporary:
            path = Path(temporary) / "providers.json"
            path.write_text(
                json.dumps(
                    {
                        "version": 1,
                        "mainProviders": ["codex", "spark"],
                        "providers": [
                            {
                                "id": "codex",
                                "agent": "gpt",
                                "defaultModel": "gpt-5.6-luna",
                                "effort": "max",
                                "maxContextTokens": 110000,
                                "modelPrefixes": ["gpt"],
                                "selectableModels": ["gpt-5.6-terra"],
                                "backend": "codex-app-server",
                            },
                            {
                                "id": "spark",
                                "agent": "spark",
                                "defaultModel": "gpt-5.3-codex-spark",
                                "effort": "xhigh",
                                "maxContextTokens": 99000,
                                "modelPrefixes": ["gpt-5.3-codex-spark"],
                                "backend": "codex-app-server",
                            },
                        ],
                    }
                ),
                encoding="utf-8",
            )
            self.assertEqual(self.resolve(str(path), "gpt-5.6-terra"), "110000")
            self.assertEqual(self.resolve(str(path), "gpt-5.3-codex-spark"), "99000")

    def resolve(self, config: str, model: str) -> str:
        result = subprocess.run(
            ["python3", str(RESOLVE), config, model],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout.strip()


if __name__ == "__main__":
    unittest.main()
