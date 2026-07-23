from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).parents[1] / "scripts"))

import route_usage  # noqa: E402


def report(codex: object = 10, grok: object = 20) -> list[dict[str, object]]:
    return [
        {"provider": "codex", "usage": {"primary": {"usedPercent": codex}}},
        {"provider": "grok", "usage": {"primary": {"usedPercent": grok}}},
    ]


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
        self.assertEqual(
            route_usage.provider_status([], "codex")["reason"], "missing"
        )
        self.assertEqual(
            route_usage.provider_status(
                [{"provider": "Codex", "usage": {}}], "codex"
            )["reason"],
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
        self.assertEqual(route_usage.cache_seconds({"CLAUDEX_USAGE_CACHE_SECONDS": "7"}), 7)
        self.assertEqual(route_usage.cache_seconds({"CLAUDEX_USAGE_CACHE_SECONDS": "-1"}), 0)
        self.assertEqual(
            route_usage.cache_seconds({"CLAUDEX_USAGE_CACHE_SECONDS": "bad"}), 300
        )


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


if __name__ == "__main__":
    unittest.main()
