from __future__ import annotations

import contextlib
import copy
import io
import json
import os
import runpy
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).parents[1] / "scripts"))

import route_usage


def qwen_report(
    available: object = True, tokens: object = 71_215, requests: object = 4
) -> dict[str, object]:
    return {
        "provider": "qwen",
        "available": available,
        "reason": (
            "available-local-stats-only" if available is True else "usage-unavailable"
        ),
        "localUsage": {
            "period": "2026-07",
            "tokens": tokens,
            "requests": requests,
        },
    }


def qwen_export(tokens: object = 71_215, requests: object = 4) -> dict[str, object]:
    return {
        "period": "month",
        "value": "2026-07",
        "totals": {"totalTokens": 108_016, "requests": 6},
        "byModel": [
            {
                "model": "qwen3.8-max-preview",
                "totalTokens": tokens,
                "requests": requests,
            }
        ],
    }


def report(
    codex: object = 10, grok: object = 20, qwen: object = True
) -> list[dict[str, object]]:
    return [
        {"provider": "codex", "usage": {"primary": {"usedPercent": codex}}},
        {"provider": "grok", "usage": {"primary": {"usedPercent": grok}}},
        qwen_report(qwen),
    ]


def configuration() -> dict[str, object]:
    return route_usage.load_config(route_usage.REPOSITORY_CONFIG)


class ConfigurationTests(unittest.TestCase):
    def test_resolves_explicit_environment_and_repository_paths(self) -> None:
        explicit = Path("explicit.json")
        self.assertEqual(route_usage.config_path({}, explicit), explicit)
        self.assertEqual(
            route_usage.config_path({"CLAUDEX_PROVIDER_CONFIG": "~/routes.json"}),
            Path.home() / "routes.json",
        )
        with mock.patch.object(Path, "is_file", return_value=False):
            self.assertEqual(route_usage.config_path({}), route_usage.REPOSITORY_CONFIG)

    def test_validates_config_structure_and_choices(self) -> None:
        base = configuration()
        invalid = []
        for key, value in [
            ("version", 2),
            ("providers", []),
            ("mainProvider", "missing"),
            ("fallback", {}),
        ]:
            changed = copy.deepcopy(base)
            changed[key] = value
            invalid.append(changed)
        changed = copy.deepcopy(base)
        changed["providers"][0]["agent"] = ""
        invalid.append(changed)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "providers.json"
            for config in invalid:
                path.write_text(json.dumps(config), encoding="utf-8")
                with self.assertRaises(ValueError):
                    route_usage.load_config(path)
        self.assertFalse(route_usage.valid_provider(None))
        self.assertFalse(route_usage.valid_choice(None))

    def test_unmetered_provider_is_available_and_config_changes_cache_key(self) -> None:
        config = configuration()
        config["providers"][0].pop("usageProvider")
        summary = route_usage.routing_summary([], config)
        self.assertEqual(summary["providers"]["codex"]["reason"], "unmetered")
        self.assertIn("claudex-gpt", summary["selected_agents"])
        original = route_usage.configuration_key(config)
        config["providers"][0]["effort"] = "xhigh"
        self.assertNotEqual(route_usage.configuration_key(config), original)


class RoutingTests(unittest.TestCase):
    def test_orchestration_artifacts_keep_delegation_as_the_standing_default(
        self,
    ) -> None:
        claude_home = Path(__file__).parents[3]
        for path in [
            claude_home / "CLAUDE.md",
            claude_home / "agents" / "claudex-orchestrator.md",
            Path(__file__).parents[1] / "SKILL.md",
        ]:
            instructions = path.read_text(encoding="utf-8")
            self.assertIn("standing default", instructions, path)
            self.assertIn("foreground", instructions, path)
            self.assertIn("N queued", instructions, path)
            self.assertNotIn("When delegation is requested", instructions, path)

        settings = json.loads(
            (claude_home / "settings.json").read_text(encoding="utf-8")
        )
        self.assertEqual(settings["advisorModel"], "opus")
        self.assertNotIn("advisor", configuration())
        self.assertFalse((claude_home / "agents" / "custom-advisor.md").exists())
        routing_hook = next(
            hook
            for group in settings["hooks"]["UserPromptSubmit"]
            for hook in group["hooks"]
            if "route_usage.py" in hook["command"]
        )
        self.assertGreater(
            routing_hook["timeout"], route_usage.USAGE_COMMAND_TIMEOUT_SECONDS
        )

    def test_collects_nested_numeric_percentages_only(self) -> None:
        usage = {
            "primary": {"usedPercent": 12},
            "extraRateWindows": [
                {"window": {"usedPercent": 34.5}},
                {"window": {"usedPercent": "ignored"}},
            ],
        }
        self.assertEqual(route_usage.usage_percentages(usage), [12.0, 34.5])
        self.assertEqual(route_usage.usage_percentages("invalid"), [])
        self.assertEqual(
            route_usage.usage_percentages(
                {
                    "boolean": {"usedPercent": True},
                    "negative": {"usedPercent": -1},
                    "infinite": {"usedPercent": float("inf")},
                }
            ),
            [],
        )

    def test_reports_missing_unknown_available_and_exhausted_providers(self) -> None:
        self.assertEqual(route_usage.provider_status([], "codex")["reason"], "missing")
        self.assertEqual(
            route_usage.provider_status([{"provider": "Codex", "usage": {}}], "codex")[
                "reason"
            ],
            "unknown",
        )
        self.assertTrue(route_usage.provider_status(report(), "codex")["available"])
        exhausted = report(codex=100)
        self.assertEqual(
            route_usage.provider_status(exhausted, "codex"),
            {
                "available": False,
                "max_used_percent": 100.0,
                "remaining_percent": 0.0,
                "reason": "exhausted",
            },
        )

    def test_reports_qwen_availability_and_local_usage(self) -> None:
        self.assertEqual(
            route_usage.provider_status(report(), "qwen"),
            {
                "available": True,
                "max_used_percent": None,
                "remaining_percent": None,
                "reason": "available-local-stats-only",
                "usage_period": "2026-07",
                "usage_tokens": 71_215,
                "usage_requests": 4,
            },
        )
        unavailable = route_usage.provider_status(report(qwen=False), "qwen")
        self.assertFalse(unavailable["available"])
        self.assertEqual(unavailable["reason"], "usage-unavailable")

    def test_rejects_malformed_explicit_usage_without_leaking_counters(self) -> None:
        malformed = qwen_report(available="yes", tokens=True, requests=-1)
        self.assertEqual(
            route_usage.provider_status([malformed], "qwen")["reason"], "unknown"
        )
        malformed["available"] = True
        status = route_usage.provider_status([malformed], "qwen")
        self.assertNotIn("usage_tokens", status)
        self.assertEqual(
            route_usage.explicitly_reported_status({"available": True})["reason"],
            "available",
        )
        self.assertFalse(route_usage.numeric_usage_value(True))
        self.assertFalse(route_usage.numeric_usage_value(1.5))

    def test_selects_all_single_and_fallback_agents(self) -> None:
        self.assertEqual(
            route_usage.routing_summary(report())["selected_agents"],
            ["claudex-gpt", "claudex-grok", "claudex-qwen"],
        )
        self.assertEqual(
            route_usage.routing_summary(report(grok=100))["selected_agents"],
            ["claudex-gpt", "claudex-qwen"],
        )
        fallback = route_usage.routing_summary(report(codex=100, grok=100, qwen=False))
        self.assertEqual(fallback["selected_agents"], ["claudex-sonnet"])
        self.assertTrue(fallback["fallback_active"])

    def test_prioritizes_the_provider_with_the_most_known_headroom(self) -> None:
        summary = route_usage.routing_summary(report(codex=80, grok=10))
        self.assertEqual(
            summary["selected_agents"],
            ["claudex-grok", "claudex-gpt", "claudex-qwen"],
        )
        self.assertEqual(summary["preferred_worker"]["provider"], "grok")
        self.assertEqual(summary["providers"]["grok"]["remaining_percent"], 90.0)
        self.assertEqual(summary["providers"]["codex"]["remaining_percent"], 20.0)
        self.assertIsNone(summary["providers"]["qwen"]["remaining_percent"])

    def test_unknown_qwen_limit_cannot_outrank_known_capacity(self) -> None:
        summary = route_usage.routing_summary(report(codex=99, grok=100))
        self.assertEqual(summary["selected_agents"], ["claudex-gpt", "claudex-qwen"])
        self.assertEqual(summary["preferred_worker"]["provider"], "codex")

    def test_hook_output_contains_only_the_sanitized_summary(self) -> None:
        summary = route_usage.routing_summary(report())
        output = route_usage.hook_output(summary)
        context = output["hookSpecificOutput"]["additionalContext"]
        self.assertEqual(
            output["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit"
        )
        self.assertIn("claudex-gpt", context)
        self.assertIn("claudex-qwen", context)
        self.assertIn("every Agent/Task launch", context)
        self.assertIn("nested launches from a worker", context)
        self.assertIn("claudex_model and claudex_effort", context)
        self.assertIn("standing default", context)
        self.assertIn("do not wait for them to repeat it", context)
        self.assertIn("built-in parameterless advisor tool", context)
        self.assertIn("complete conversation history", context)
        self.assertNotIn("custom-advisor", context)
        self.assertIn("TUI N queued", context)
        self.assertIn("not worker capacity", context)
        self.assertNotIn("account", context)


class CacheTests(unittest.TestCase):
    def test_cache_round_trip_expiration_and_disable(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "routing.json"
            summary = route_usage.routing_summary(report())
            route_usage.write_cache(path, summary, 100)
            self.assertEqual(route_usage.read_cache(path, 105, 10), summary)
            self.assertIsNone(route_usage.read_cache(path, 111, 10))
            self.assertIsNone(route_usage.read_cache(path, 105, 0))
            self.assertIsNone(route_usage.read_cache(path, 105, 10, "different"))
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)

    def test_ignores_missing_malformed_and_incomplete_cache(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "routing.json"
            self.assertIsNone(route_usage.read_cache(path, 1, 10))
            path.write_text("not-json", encoding="utf-8")
            self.assertIsNone(route_usage.read_cache(path, 1, 10))
            path.write_text(json.dumps({"created_at": "bad"}), encoding="utf-8")
            self.assertIsNone(route_usage.read_cache(path, 1, 10))

    def test_cache_seconds_handles_valid_invalid_and_negative_values(self) -> None:
        self.assertEqual(route_usage.cache_seconds({}), 300)
        self.assertEqual(
            route_usage.cache_seconds({"CLAUDEX_USAGE_CACHE_SECONDS": "7"}), 7
        )
        self.assertEqual(
            route_usage.cache_seconds({"CLAUDEX_USAGE_CACHE_SECONDS": "-1"}), 0
        )
        self.assertEqual(
            route_usage.cache_seconds({"CLAUDEX_USAGE_CACHE_SECONDS": "bad"}), 300
        )

    @mock.patch("route_usage.tempfile.NamedTemporaryFile", side_effect=OSError("write"))
    def test_cache_write_preserves_the_original_error_before_creation(
        self, _temporary: mock.Mock
    ) -> None:
        with self.assertRaisesRegex(OSError, "write"):
            route_usage.write_cache(Path("unused"), {}, 1)


class CommandTests(unittest.TestCase):
    def test_converts_a_recorded_qwen_0_21_monthly_export(self) -> None:
        fixture = Path(__file__).parent / "fixtures" / "qwen-monthly.json"
        payload = json.loads(fixture.read_text(encoding="utf-8"))
        entry = route_usage.qwen_usage_entry(payload, "qwen", "qwen3.8-max-preview")
        self.assertEqual(
            entry,
            qwen_report(tokens=92_493, requests=5),
        )

    @mock.patch("route_usage.subprocess.run")
    def test_runs_codexbar_without_a_shell(self, run: mock.Mock) -> None:
        run.return_value = subprocess.CompletedProcess([], 0, json.dumps(report()), "")
        self.assertEqual(route_usage.run_codexbar("codexbar-test"), report())
        run.assert_called_once_with(
            ["codexbar-test", "usage", "--json"],
            check=True,
            capture_output=True,
            text=True,
            timeout=route_usage.USAGE_COMMAND_TIMEOUT_SECONDS,
        )

    @mock.patch("route_usage.subprocess.run")
    def test_runs_qwen_local_monthly_export_without_a_model_prompt(
        self, run: mock.Mock
    ) -> None:
        def write_export(
            *_args: object, **kwargs: object
        ) -> subprocess.CompletedProcess:
            Path(str(kwargs["cwd"]), route_usage.QWEN_USAGE_FILENAME).write_text(
                json.dumps(qwen_export()), encoding="utf-8"
            )
            return subprocess.CompletedProcess([], 0, "exported", "")

        run.side_effect = write_export
        entry = route_usage.run_qwen_usage("qwen-test", "qwen", "qwen3.8-max-preview")
        self.assertEqual(entry, qwen_report())
        arguments = run.call_args.args[0]
        self.assertEqual(arguments[0], "qwen-test")
        self.assertTrue(
            any(argument.startswith("/stats export monthly") for argument in arguments)
        )
        self.assertIn("--safe-mode", arguments)
        self.assertEqual(run.call_args.kwargs["timeout"], 45)

    def test_validates_qwen_export_and_zeroes_a_new_model(self) -> None:
        entry = route_usage.qwen_usage_entry(qwen_export(), "qwen", "new-model")
        self.assertEqual(entry["localUsage"]["tokens"], 0)
        invalid = [
            [],
            {},
            {"period": "day", "value": "2026-07", "byModel": []},
            {"period": "month", "value": "July 2026", "byModel": []},
            {"value": "2026-07", "byModel": "invalid"},
            qwen_export(tokens="invalid"),
        ]
        for payload in invalid:
            with self.assertRaises((TypeError, ValueError)):
                route_usage.qwen_usage_entry(payload, "qwen", "qwen3.8-max-preview")

    @mock.patch("route_usage.run_qwen_usage", return_value=qwen_report())
    @mock.patch("route_usage.run_codexbar", return_value=report()[:2])
    def test_collects_codexbar_and_qwen_usage(
        self, _codexbar: mock.Mock, qwen: mock.Mock
    ) -> None:
        collected = route_usage.collect_usage(configuration(), "codexbar", "qwen")
        self.assertEqual(
            [entry["provider"] for entry in collected], ["codex", "grok", "qwen"]
        )
        qwen.assert_called_once_with("qwen", "qwen", "qwen3.8-max-preview")

    @mock.patch("route_usage.run_qwen_usage", return_value=qwen_report())
    @mock.patch(
        "route_usage.run_codexbar",
        return_value=[*report()[:2], {"provider": "qwen", "usage": {}}],
    )
    def test_qwen_cli_usage_replaces_a_codexbar_qwen_entry(
        self, _codexbar: mock.Mock, _qwen: mock.Mock
    ) -> None:
        collected = route_usage.collect_usage(configuration(), "codexbar", "qwen")
        qwen_entries = [entry for entry in collected if entry.get("provider") == "qwen"]
        self.assertEqual(qwen_entries, [qwen_report()])
        qwen_worker = next(
            worker
            for worker in route_usage.routing_summary(collected)["selected_workers"]
            if worker["provider"] == "qwen"
        )
        self.assertEqual(qwen_worker["model"], "qwen3.8-max-preview")
        self.assertEqual(qwen_worker["model_prefixes"], ["qwen"])

    @mock.patch("route_usage.run_qwen_usage", side_effect=OSError("missing"))
    @mock.patch("route_usage.run_codexbar", return_value=report()[:2])
    def test_qwen_usage_failure_does_not_disable_codexbar_providers(
        self, _codexbar: mock.Mock, _qwen: mock.Mock
    ) -> None:
        collected = route_usage.collect_usage(configuration(), "codexbar", "qwen")
        summary = route_usage.routing_summary(collected)
        self.assertEqual(summary["selected_agents"], ["claudex-gpt", "claudex-grok"])
        self.assertEqual(summary["providers"]["qwen"]["reason"], "usage-unavailable")

    @mock.patch("route_usage.run_qwen_usage", return_value=qwen_report())
    @mock.patch("route_usage.run_codexbar", side_effect=OSError("missing"))
    def test_codexbar_failure_does_not_disable_qwen(
        self, _codexbar: mock.Mock, _qwen: mock.Mock
    ) -> None:
        collected = route_usage.collect_usage(configuration(), "codexbar", "qwen")
        summary = route_usage.routing_summary(collected)
        self.assertEqual(summary["selected_agents"], ["claudex-qwen"])
        self.assertEqual(summary["providers"]["codex"]["reason"], "usage-unavailable")

    def test_fallback_summary_disables_external_providers(self) -> None:
        summary = route_usage.fallback_summary("test-failure")
        self.assertEqual(summary["selected_agents"], ["claudex-sonnet"])
        self.assertTrue(summary["fallback_active"])
        self.assertEqual(summary["providers"]["codex"]["reason"], "test-failure")

    def test_cli_fixture_and_failure_paths_emit_valid_hook_json(self) -> None:
        script = Path(route_usage.__file__)
        with tempfile.TemporaryDirectory() as directory:
            fixture = Path(directory) / "usage.json"
            fixture.write_text(json.dumps(report()), encoding="utf-8")
            success = subprocess.run(
                [sys.executable, str(script), "--input", str(fixture)],
                check=True,
                capture_output=True,
                text=True,
            )
            self.assertIn(
                "claudex-gpt",
                json.loads(success.stdout)["hookSpecificOutput"]["additionalContext"],
            )

            failure = subprocess.run(
                [
                    sys.executable,
                    str(script),
                    "--no-cache",
                    "--codexbar-program",
                    str(Path(directory) / "missing"),
                ],
                check=True,
                capture_output=True,
                text=True,
                env={**os.environ, "HOME": directory},
            )
            self.assertIn(
                "claudex-sonnet",
                json.loads(failure.stdout)["hookSpecificOutput"]["additionalContext"],
            )


class MainTests(unittest.TestCase):
    def test_parses_every_cli_option(self) -> None:
        with mock.patch.object(
            sys,
            "argv",
            [
                "route_usage.py",
                "--config",
                "providers.json",
                "--input",
                "usage.json",
                "--no-cache",
                "--codexbar-program",
                "usage-tool",
                "--qwen-program",
                "qwen-tool",
            ],
        ):
            arguments = route_usage.parse_arguments()
        self.assertEqual(arguments.input, Path("usage.json"))
        self.assertEqual(arguments.config, Path("providers.json"))
        self.assertTrue(arguments.no_cache)
        self.assertEqual(arguments.codexbar_program, "usage-tool")
        self.assertEqual(arguments.qwen_program, "qwen-tool")

    @mock.patch("route_usage.collect_usage")
    @mock.patch("route_usage.read_cache")
    def test_main_reuses_a_fresh_cache(
        self, read_cache: mock.Mock, collect_usage: mock.Mock
    ) -> None:
        read_cache.return_value = route_usage.routing_summary(report())
        output = self.run_main()
        self.assertIn("claudex-gpt", output)
        collect_usage.assert_not_called()

    @mock.patch("route_usage.write_cache")
    @mock.patch("route_usage.collect_usage", return_value=report())
    @mock.patch("route_usage.read_cache", return_value=None)
    def test_main_refreshes_and_writes_the_cache(
        self,
        _read_cache: mock.Mock,
        _collect_usage: mock.Mock,
        write_cache: mock.Mock,
    ) -> None:
        output = self.run_main()
        self.assertIn("claudex-qwen", output)
        write_cache.assert_called_once()

    @mock.patch("route_usage.write_cache")
    @mock.patch("route_usage.collect_usage")
    @mock.patch("route_usage.read_cache", return_value=None)
    def test_main_reads_an_uncached_fixture(
        self,
        _read_cache: mock.Mock,
        collect_usage: mock.Mock,
        write_cache: mock.Mock,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Path(directory) / "usage.json"
            fixture.write_text(json.dumps(report(grok=100)), encoding="utf-8")
            output = self.run_main("--input", str(fixture))
        context = json.loads(output)["hookSpecificOutput"]["additionalContext"]
        self.assertIn('"selected_agents":["claudex-gpt","claudex-qwen"]', context)
        collect_usage.assert_not_called()
        write_cache.assert_not_called()

    @mock.patch("route_usage.collect_usage", side_effect=OSError("failed"))
    @mock.patch("route_usage.read_cache", return_value=None)
    def test_main_falls_back_when_usage_refresh_fails(
        self, _read_cache: mock.Mock, _collect_usage: mock.Mock
    ) -> None:
        output = self.run_main("--no-cache")
        self.assertIn("usage-unavailable", output)
        self.assertIn("claudex-sonnet", output)

    def test_module_entrypoint_exits_with_main_status(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Path(directory) / "usage.json"
            fixture.write_text(json.dumps(report()), encoding="utf-8")
            stdout = io.StringIO()
            with (
                mock.patch.object(
                    sys,
                    "argv",
                    [str(Path(route_usage.__file__)), "--input", str(fixture)],
                ),
                contextlib.redirect_stdout(stdout),
                self.assertRaises(SystemExit) as exit_status,
            ):
                runpy.run_path(str(Path(route_usage.__file__)), run_name="__main__")
        self.assertEqual(exit_status.exception.code, 0)
        self.assertIn("claudex-gpt", stdout.getvalue())

    def test_main_rejects_an_invalid_configuration(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "providers.json"
            path.write_text("{}", encoding="utf-8")
            with self.assertRaisesRegex(SystemExit, "configuration error"):
                self.run_main("--config", str(path))

    def run_main(self, *arguments: str) -> str:
        stdout = io.StringIO()
        with (
            mock.patch.object(
                sys, "argv", [str(Path(route_usage.__file__)), *arguments]
            ),
            contextlib.redirect_stdout(stdout),
        ):
            self.assertEqual(route_usage.main(), 0)
        return stdout.getvalue()


if __name__ == "__main__":
    unittest.main()
