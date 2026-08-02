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
    def test_direnv_does_not_override_standalone_claude_authentication(self) -> None:
        with tempfile.TemporaryDirectory(prefix="claudex-direnv-auth-") as temporary:
            workdir = Path(temporary)
            (workdir / ".env").write_text(
                "\n".join(
                    [
                        "ANTHROPIC_BASE_URL=https://gateway.invalid/remote",
                        "ANTHROPIC_AUTH_TOKEN=remote-token",
                        "ANTHROPIC_API_KEY=remote-key",
                        "ANTHROPIC_CUSTOM_HEADERS=Authorization: Bearer remote-token",
                        "BRAVE_API_KEY=keep-for-other-tools",
                    ]
                )
                + "\n",
                encoding="utf-8",
            )
            result = subprocess.run(
                [
                    "bash",
                    "-c",
                    (
                        "dotenv() { set -a; . .env; set +a; }; "
                        f"source '{ROOT / '.envrc'}'; "
                        "env | sort | grep -E '^(ANTHROPIC_|BRAVE_API_KEY=)' || true"
                    ),
                ],
                cwd=workdir,
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertNotRegex(result.stdout, r"(?m)^ANTHROPIC_(?:BASE_URL|CUSTOM_HEADERS)=")
            self.assertIn("ANTHROPIC_AUTH_TOKEN=remote-token", result.stdout)
            self.assertIn("ANTHROPIC_API_KEY=remote-key", result.stdout)
            self.assertIn("BRAVE_API_KEY=keep-for-other-tools", result.stdout)

    def test_env_example_keeps_anthropic_gateway_opt_in(self) -> None:
        example = (ROOT / ".env.example").read_text(encoding="utf-8")
        self.assertNotRegex(example, r"(?m)^ANTHROPIC_(?:BASE_URL|AUTH_TOKEN|API_KEY)=")
        self.assertIn("Opt into a remote gateway", example)

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
            if line.startswith(("CLAUDE_CODE_MAX_", "CLAUDEX_SUBAGENT_"))
        )
        self.assertEqual(output["CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS"], "40")
        self.assertEqual(output["CLAUDEX_SUBAGENT_MAX_PARALLEL"], "40")
        self.assertEqual(output["CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION"], "1024")
        for obsolete in (
            "CLAUDEX_SUBAGENT_MIN_PARALLEL",
            "CLAUDEX_SUBAGENT_ACTIVE_FLOOR",
            "CLAUDEX_SUBAGENT_MIN_MODEL_FAMILIES",
        ):
            self.assertNotIn(obsolete, output)
        self.assertEqual(output["CLAUDEX_SUBAGENT_FIRST"], "1")
        self.assertEqual(output["CLAUDEX_SUBAGENT_STATUS_POLL_SECONDS"], "15")

    def test_defaults_are_exported_to_claude_child(self) -> None:
        output = self.run_launcher({})

        self.assertEqual(output["CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS"], "40")
        self.assertEqual(output["CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION"], "1024")
        self.assertEqual(output["CLAUDEX_SUBAGENT_MAX_PARALLEL"], "40")
        self.assertEqual(output["CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS"], "600")
        self.assertEqual(output["CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION"], "1")
        self.assertEqual(output["CLAUDEX_SUBAGENT_REUSE"], "1")
        self.assertEqual(output["CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT"], "1")
        self.assertEqual(output["CLAUDEX_SUBAGENT_FIRST"], "1")
        self.assertEqual(output["CLAUDEX_SUBAGENT_STATUS_POLL_SECONDS"], "15")
        self.assertNotIn("--agent claudex-orchestrator", output["CLAUDEX_ADAPTER_ARGS"])
        self.assertEqual(output["CLAUDEX_SUBSCRIPTION_MAX_PROCESSES"], "20")
        self.assertEqual(output["CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES"], "120")
        self.assertEqual(output["CLAUDEX_OUTER_MODEL"], "sonnet[1m]")
        self.assertEqual(output["CLAUDEX_MAIN_MODEL"], "sonnet[1m]")
        self.assertEqual(output["CLAUDEX_MAIN_MODEL_KNOWN"], "1")
        self.assertNotIn("--model", output["CLAUDEX_ADAPTER_ARGS"])
        self.assertIn("--inherit-claude-model", output["CLAUDEX_ADAPTER_ARGS"])

    def test_explicit_sonnet_outer_model_is_forwarded_without_changing_worker_definition(
        self,
    ) -> None:
        output = self.run_launcher({"CLAUDEX_MODEL": "claude-sonnet-5"})
        self.assertEqual(output["CLAUDEX_OUTER_MODEL"], "claude-sonnet-5")
        self.assertEqual(output["CLAUDEX_MAIN_MODEL"], "claude-sonnet-5")
        self.assertEqual(output["CLAUDEX_MAIN_MODEL_KNOWN"], "1")
        self.assertIn("--model claude-sonnet-5", output["CLAUDEX_ADAPTER_ARGS"])
        self.assertNotIn("--inherit-claude-model", output["CLAUDEX_ADAPTER_ARGS"])

    def test_resume_does_not_claim_settings_model_is_restored_model(self) -> None:
        output = self.run_launcher({}, ["--resume", "saved-session", "continue"])
        self.assertEqual(output["CLAUDEX_OUTER_MODEL"], "sonnet[1m]")
        self.assertEqual(output["CLAUDEX_MAIN_MODEL"], "")
        self.assertEqual(output["CLAUDEX_MAIN_MODEL_KNOWN"], "0")
        self.assertIn("--inherit-claude-model", output["CLAUDEX_ADAPTER_ARGS"])

    def test_continue_does_not_claim_settings_model_is_restored_model(self) -> None:
        output = self.run_launcher({}, ["--continue", "continue"])
        self.assertEqual(output["CLAUDEX_OUTER_MODEL"], "sonnet[1m]")
        self.assertEqual(output["CLAUDEX_MAIN_MODEL"], "")
        self.assertEqual(output["CLAUDEX_MAIN_MODEL_KNOWN"], "0")

    def test_explicit_model_remains_known_when_resuming(self) -> None:
        output = self.run_launcher(
            {"CLAUDEX_MODEL": "grok-4.5"},
            ["--resume", "saved-session", "continue"],
        )
        self.assertEqual(output["CLAUDEX_MAIN_MODEL"], "grok-4.5")
        self.assertEqual(output["CLAUDEX_MAIN_MODEL_KNOWN"], "1")

    def test_external_values_override_defaults_without_shell_evaluation(self) -> None:
        values = {
            "CLAUDEX_SUBAGENT_MAX_PARALLEL": "7",
            "CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS": "120",
            "CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION": "0",
            "CLAUDEX_SUBAGENT_REUSE": "0",
            "CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT": "0",
            "CLAUDEX_SUBAGENT_FIRST": "0",
            "CLAUDEX_SUBAGENT_STATUS_POLL_SECONDS": "7",
            "CLAUDEX_SUBSCRIPTION_MAX_PROCESSES": "8",
            "CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES": "60",
        }
        output = self.run_launcher(values)

        for name, value in values.items():
            self.assertEqual(output[name], value)
        self.assertEqual(output["CLAUDEX_SUBSCRIPTION_MAX_PROCESSES"], "8")
        self.assertEqual(output["CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES"], "60")
        self.assertEqual(output["CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS"], "7")

    def run_launcher(
        self,
        environment: dict[str, str],
        arguments: list[str] | None = None,
    ) -> dict[str, str]:
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
                "adapter_args=\"$*\"\n"
                "while [ \"$#\" -gt 0 ]; do\n"
                "  case \"$1\" in\n"
                "    --subscription-max-processes) shift; subscription_max_processes=$1 ;;\n"
                "    --subscription-timeout-minutes) shift; subscription_timeout_minutes=$1 ;;\n"
                "  esac\n"
                "  shift\n"
                "done\n"
                "printf 'CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS=%s\\n' "
                "\"${CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS:-}\"\n"
                "printf 'CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION=%s\\n' "
                "\"${CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION:-}\"\n"
                "printf 'CLAUDEX_SUBAGENT_MAX_PARALLEL=%s\\n' "
                "\"${CLAUDEX_SUBAGENT_MAX_PARALLEL:-}\"\n"
                "printf 'CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS=%s\\n' "
                "\"${CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS:-}\"\n"
                "printf 'CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION=%s\\n' "
                "\"${CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION:-}\"\n"
                "printf 'CLAUDEX_SUBAGENT_REUSE=%s\\n' "
                "\"${CLAUDEX_SUBAGENT_REUSE:-}\"\n"
                "printf 'CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT=%s\\n' "
                "\"${CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT:-}\"\n"
                "printf 'CLAUDEX_SUBAGENT_FIRST=%s\\n' "
                "\"${CLAUDEX_SUBAGENT_FIRST:-}\"\n"
                "printf 'CLAUDEX_SUBAGENT_STATUS_POLL_SECONDS=%s\\n' "
                "\"${CLAUDEX_SUBAGENT_STATUS_POLL_SECONDS:-}\"\n"
                "printf 'CLAUDEX_ADAPTER_ARGS=%s\\n' \"${adapter_args}\"\n"
                "printf 'CLAUDEX_OUTER_MODEL=%s\\n' "
                "\"${CLAUDEX_OUTER_MODEL:-}\"\n"
                "printf 'CLAUDEX_MAIN_MODEL=%s\\n' "
                "\"${CLAUDEX_MAIN_MODEL:-}\"\n"
                "printf 'CLAUDEX_MAIN_MODEL_KNOWN=%s\\n' "
                "\"${CLAUDEX_MAIN_MODEL_KNOWN:-}\"\n"
                "printf 'CLAUDEX_SUBSCRIPTION_MAX_PROCESSES=%s\\n' "
                "\"${subscription_max_processes}\"\n"
                "printf 'CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES=%s\\n' "
                "\"${subscription_timeout_minutes}\"\n",
                encoding="utf-8",
            )
            adapter.chmod(adapter.stat().st_mode | stat.S_IXUSR)
            launcher_arguments = arguments or ["orchestration-smoke"]
            command = [
                "fish",
                "--no-config",
                "-c",
                f"source '{FUNCTION}'; claudex {' '.join(launcher_arguments)}",
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
