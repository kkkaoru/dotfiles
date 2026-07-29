#!/usr/bin/env python3
"""Smoke-test the fish launcher orchestration environment contract."""

from __future__ import annotations

import json
from pathlib import Path
import os
import stat
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[3]
FUNCTION = ROOT / ".config/fish/functions/claudex.fish"


class ClaudexOrchestrationEnvironmentTests(unittest.TestCase):
    def test_routing_keeps_two_model_families_and_advisor_separate(self) -> None:
        configuration = json.loads(
            (ROOT / ".config/claudex/providers.json").read_text(encoding="utf-8")
        )
        worker_models = [
            provider.get("subagentModel", provider.get("defaultModel"))
            for provider in configuration["providers"]
            if provider.get("enabled", True)
        ]
        families = {str(model).split("-", 1)[0] for model in worker_models}
        self.assertGreaterEqual(len(families), 2)
        advisor = configuration["advisor"]["model"]
        self.assertNotIn(advisor, worker_models)
        self.assertEqual(configuration["advisor"]["agent"], "custom-advisor")

    def test_plain_fish_config_exports_the_same_policy_defaults(self) -> None:
        result = subprocess.run(
            [
                "fish",
                "--no-config",
                "-c",
                f"source '{ROOT / '.config/fish/config.fish'}'; env",
            ],
            cwd=ROOT,
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        output = dict(
            line.split("=", 1)
            for line in result.stdout.splitlines()
            if line.startswith(("CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS=", "CLAUDEX_SUBAGENT_"))
        )
        self.assertEqual(output["CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS"], "40")
        self.assertEqual(output["CLAUDEX_SUBAGENT_MAX_PARALLEL"], "40")
        self.assertEqual(output["CLAUDEX_SUBAGENT_MIN_PARALLEL"], "3")
        self.assertEqual(output["CLAUDEX_SUBAGENT_MIN_MODEL_FAMILIES"], "2")

    def test_defaults_are_exported_to_claude_child(self) -> None:
        output = self.run_launcher({})

        self.assertEqual(output["CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS"], "40")
        self.assertEqual(output["CLAUDEX_SUBAGENT_MIN_PARALLEL"], "3")
        self.assertEqual(output["CLAUDEX_SUBAGENT_ACTIVE_FLOOR"], "2")
        self.assertEqual(output["CLAUDEX_SUBAGENT_MIN_MODEL_FAMILIES"], "2")
        self.assertEqual(output["CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS"], "600")
        self.assertEqual(output["CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION"], "1")
        self.assertEqual(output["CLAUDEX_SUBAGENT_REUSE"], "1")
        self.assertEqual(output["CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT"], "1")
        self.assertEqual(output["CLAUDEX_SUBSCRIPTION_MAX_PROCESSES"], "20")
        self.assertEqual(output["CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES"], "120")

    def test_external_values_override_defaults_without_shell_evaluation(self) -> None:
        values = {
            "CLAUDEX_SUBAGENT_MAX_PARALLEL": "7",
            "CLAUDEX_SUBAGENT_MIN_PARALLEL": "4",
            "CLAUDEX_SUBAGENT_ACTIVE_FLOOR": "3",
            "CLAUDEX_SUBAGENT_MIN_MODEL_FAMILIES": "3",
            "CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS": "120",
            "CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION": "0",
            "CLAUDEX_SUBAGENT_REUSE": "0",
            "CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT": "0",
            "CLAUDEX_SUBSCRIPTION_MAX_PROCESSES": "8",
            "CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES": "60",
        }
        output = self.run_launcher(values)

        for name, value in values.items():
            self.assertEqual(output[name], value)
        self.assertEqual(output["CLAUDEX_SUBSCRIPTION_MAX_PROCESSES"], "8")
        self.assertEqual(output["CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES"], "60")
        self.assertEqual(output["CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS"], "7")

    def run_launcher(self, environment: dict[str, str]) -> dict[str, str]:
        with tempfile.TemporaryDirectory(prefix="claudex-fish-smoke-") as temporary:
            home = Path(temporary)
            (home / ".config/claudex").mkdir(parents=True)
            (home / ".local/bin").mkdir(parents=True)
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
                        "fallback": {
                            "agent": "fallback",
                            "model": "sonnet",
                            "effort": "high",
                        },
                    }
                ),
                encoding="utf-8",
            )
            (home / ".claude/settings.json").write_text(
                '{"model":"sonnet[1m]","effortLevel":"high"}',
                encoding="utf-8",
            )
            adapter = home / ".local/bin/claudex-agent-adapter"
            adapter.write_text(
                "#!/bin/sh\n"
                "subscription_max_processes=\n"
                "subscription_timeout_minutes=\n"
                "while [ \"$#\" -gt 0 ]; do\n"
                "  case \"$1\" in\n"
                "    --subscription-max-processes) shift; subscription_max_processes=$1 ;;\n"
                "    --subscription-timeout-minutes) shift; subscription_timeout_minutes=$1 ;;\n"
                "  esac\n"
                "  shift\n"
                "done\n"
                "printf 'CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS=%s\\n' "
                "\"${CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS:-}\"\n"
                "printf 'CLAUDEX_SUBAGENT_MAX_PARALLEL=%s\\n' "
                "\"${CLAUDEX_SUBAGENT_MAX_PARALLEL:-}\"\n"
                "printf 'CLAUDEX_SUBAGENT_MIN_PARALLEL=%s\\n' "
                "\"${CLAUDEX_SUBAGENT_MIN_PARALLEL:-}\"\n"
                "printf 'CLAUDEX_SUBAGENT_ACTIVE_FLOOR=%s\\n' "
                "\"${CLAUDEX_SUBAGENT_ACTIVE_FLOOR:-}\"\n"
                "printf 'CLAUDEX_SUBAGENT_MIN_MODEL_FAMILIES=%s\\n' "
                "\"${CLAUDEX_SUBAGENT_MIN_MODEL_FAMILIES:-}\"\n"
                "printf 'CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS=%s\\n' "
                "\"${CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS:-}\"\n"
                "printf 'CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION=%s\\n' "
                "\"${CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION:-}\"\n"
                "printf 'CLAUDEX_SUBAGENT_REUSE=%s\\n' "
                "\"${CLAUDEX_SUBAGENT_REUSE:-}\"\n"
                "printf 'CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT=%s\\n' "
                "\"${CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT:-}\"\n"
                "printf 'CLAUDEX_SUBSCRIPTION_MAX_PROCESSES=%s\\n' "
                "\"${subscription_max_processes}\"\n"
                "printf 'CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES=%s\\n' "
                "\"${subscription_timeout_minutes}\"\n",
                encoding="utf-8",
            )
            adapter.chmod(adapter.stat().st_mode | stat.S_IXUSR)
            command = [
                "fish",
                "--no-config",
                "-c",
                f"source '{FUNCTION}'; claudex orchestration-smoke",
            ]
            result = subprocess.run(
                command,
                cwd=ROOT,
                env={**os.environ, **environment, "HOME": str(home)},
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            return dict(
                line.split("=", 1)
                for line in result.stdout.splitlines()
                if "=" in line
            )


if __name__ == "__main__":
    unittest.main()
