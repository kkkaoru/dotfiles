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


def report(codex: object = 10, grok: object = 20) -> list[dict[str, object]]:
    return [
        {"provider": "codex", "usage": {"primary": {"usedPercent": codex}}},
        {"provider": "grok", "usage": {"primary": {"usedPercent": grok}}},
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
            ("advisor", {}),
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
            {"available": False, "max_used_percent": 100.0, "reason": "exhausted"},
        )

    def test_selects_both_single_and_fallback_agents(self) -> None:
        self.assertEqual(
            route_usage.routing_summary(report())["selected_agents"],
            ["claudex-gpt", "claudex-grok"],
        )
        self.assertEqual(
            route_usage.routing_summary(report(grok=100))["selected_agents"],
            ["claudex-gpt"],
        )
        fallback = route_usage.routing_summary(report(codex=100, grok=100))
        self.assertEqual(fallback["selected_agents"], ["claudex-sonnet"])
        self.assertTrue(fallback["fallback_active"])

    def test_hook_output_contains_only_the_sanitized_summary(self) -> None:
        summary = route_usage.routing_summary(report())
        output = route_usage.hook_output(summary)
        context = output["hookSpecificOutput"]["additionalContext"]
        self.assertEqual(
            output["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit"
        )
        self.assertIn("claudex-gpt", context)
        self.assertIn("every Agent/Task launch", context)
        self.assertIn("nested launches from a worker", context)
        self.assertIn("claudex_model and claudex_effort", context)
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
    @mock.patch("route_usage.subprocess.run")
    def test_runs_codexbar_without_a_shell(self, run: mock.Mock) -> None:
        run.return_value = subprocess.CompletedProcess([], 0, json.dumps(report()), "")
        self.assertEqual(route_usage.run_codexbar("codexbar-test"), report())
        run.assert_called_once_with(
            ["codexbar-test", "usage", "--json"],
            check=True,
            capture_output=True,
            text=True,
            timeout=45,
        )

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
            ],
        ):
            arguments = route_usage.parse_arguments()
        self.assertEqual(arguments.input, Path("usage.json"))
        self.assertEqual(arguments.config, Path("providers.json"))
        self.assertTrue(arguments.no_cache)
        self.assertEqual(arguments.codexbar_program, "usage-tool")

    @mock.patch("route_usage.run_codexbar")
    @mock.patch("route_usage.read_cache")
    def test_main_reuses_a_fresh_cache(
        self, read_cache: mock.Mock, run_codexbar: mock.Mock
    ) -> None:
        read_cache.return_value = route_usage.routing_summary(report())
        output = self.run_main()
        self.assertIn("claudex-gpt", output)
        run_codexbar.assert_not_called()

    @mock.patch("route_usage.write_cache")
    @mock.patch("route_usage.run_codexbar", return_value=report())
    @mock.patch("route_usage.read_cache", return_value=None)
    def test_main_refreshes_and_writes_the_cache(
        self,
        _read_cache: mock.Mock,
        _run_codexbar: mock.Mock,
        write_cache: mock.Mock,
    ) -> None:
        output = self.run_main()
        self.assertIn("claudex-grok", output)
        write_cache.assert_called_once()

    @mock.patch("route_usage.write_cache")
    @mock.patch("route_usage.run_codexbar")
    @mock.patch("route_usage.read_cache", return_value=None)
    def test_main_reads_an_uncached_fixture(
        self,
        _read_cache: mock.Mock,
        run_codexbar: mock.Mock,
        write_cache: mock.Mock,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Path(directory) / "usage.json"
            fixture.write_text(json.dumps(report(grok=100)), encoding="utf-8")
            output = self.run_main("--input", str(fixture))
        context = json.loads(output)["hookSpecificOutput"]["additionalContext"]
        self.assertIn('"selected_agents":["claudex-gpt"]', context)
        run_codexbar.assert_not_called()
        write_cache.assert_not_called()

    @mock.patch(
        "route_usage.run_codexbar",
        side_effect=subprocess.TimeoutExpired("codexbar", 45),
    )
    @mock.patch("route_usage.read_cache", return_value=None)
    def test_main_falls_back_when_usage_refresh_fails(
        self, _read_cache: mock.Mock, _run_codexbar: mock.Mock
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
