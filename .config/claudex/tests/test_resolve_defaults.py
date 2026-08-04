#!/usr/bin/env python3

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
RESOLVER = ROOT / "resolve-defaults.py"


class ResolveDefaultsTests(unittest.TestCase):
    def run_resolver(
        self,
        defaults: dict | None,
        settings: dict,
        env: dict[str, str] | None = None,
    ) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as temp:
            temp_path = Path(temp)
            settings_path = temp_path / "settings.json"
            settings_path.write_text(json.dumps(settings), encoding="utf-8")
            if defaults is None:
                defaults_arg = "-"
            else:
                defaults_path = temp_path / "defaults.local.json"
                defaults_path.write_text(json.dumps(defaults), encoding="utf-8")
                defaults_arg = str(defaults_path)
            command_env = os.environ.copy()
            for key in ("CLAUDEX_MODEL", "CLAUDEX_EFFORT", "CLAUDEX_DEFAULTS_SOURCE"):
                command_env.pop(key, None)
            if env:
                command_env.update(env)
            return subprocess.run(
                [sys.executable, str(RESOLVER), defaults_arg, str(settings_path)],
                check=False,
                capture_output=True,
                text=True,
                env=command_env,
            )

    def test_missing_defaults_inherits_settings(self) -> None:
        result = self.run_resolver(
            None,
            {"model": "sonnet[1m]", "effortLevel": "high"},
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            result.stdout.splitlines(),
            ["settings", "sonnet[1m]", "high", "sonnet[1m]", "high"],
        )

    def test_local_defaults_without_source_are_explicit(self) -> None:
        result = self.run_resolver(
            {"version": 1, "model": "fugu", "effort": "high"},
            {"model": "sonnet[1m]", "effortLevel": "medium"},
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            result.stdout.splitlines(),
            ["explicit", "fugu", "high", "", ""],
        )

    def test_local_defaults_can_opt_into_settings(self) -> None:
        result = self.run_resolver(
            {
                "version": 1,
                "source": "settings",
            },
            {"model": "sonnet[1m]", "effortLevel": "high"},
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            result.stdout.splitlines(),
            ["settings", "sonnet[1m]", "high", "sonnet[1m]", "high"],
        )

    def test_settings_source_applies_local_model_and_effort_overrides(self) -> None:
        result = self.run_resolver(
            {
                "version": 1,
                "source": "settings",
                "model": "fugu",
                "effort": "max",
            },
            {"model": "sonnet[1m]", "effortLevel": "high"},
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            result.stdout.splitlines(),
            ["settings", "fugu", "max", "fugu", "max"],
        )

    def test_settings_source_allows_partial_effort_override(self) -> None:
        result = self.run_resolver(
            {
                "version": 1,
                "source": "settings",
                "effort": "low",
            },
            {"model": "sonnet[1m]", "effortLevel": "high"},
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            result.stdout.splitlines(),
            ["settings", "sonnet[1m]", "low", "sonnet[1m]", "low"],
        )


if __name__ == "__main__":
    unittest.main()
