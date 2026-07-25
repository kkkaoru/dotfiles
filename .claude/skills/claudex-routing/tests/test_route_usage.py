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
from urllib.parse import urlencode

sys.path.insert(0, str(Path(__file__).parents[1] / "scripts"))

import route_usage


def qwen_report(
    used: object = 5, available: object = True, reason: str | None = None
) -> dict[str, object]:
    result: dict[str, object] = {
        "provider": "qwen",
        "available": available,
        "reason": reason
        or ("available-qwen-cloud-quota" if available is True else "usage-unavailable"),
    }
    if used is not None:
        result["maxUsedPercent"] = used
    return result


def quota_payload(
    five_hour: object = 0.01,
    seven_day: object = 0.02,
    five_hour_reset: object = 1_785_000_000_000,
    seven_day_reset: object = 1_786_000_000_000,
) -> dict[str, object]:
    return {
        "data": {
            "DataV2": {
                "data": {
                    "data": {
                        "per5HourPercentage": five_hour,
                        "per1WeekPercentage": seven_day,
                        "per5HourResetTime": five_hour_reset,
                        "per1WeekResetTime": seven_day_reset,
                    }
                }
            }
        }
    }


def report(
    codex: object = 10, grok: object = 20, qwen_used: object = 5
) -> list[dict[str, object]]:
    return [
        {"provider": "codex", "usage": {"primary": {"usedPercent": codex}}},
        {"provider": "grok", "usage": {"primary": {"usedPercent": grok}}},
        qwen_report(qwen_used),
    ]


def qwen_curl_text(**overrides: str) -> str:
    params = json.dumps(
        {
            "Api": route_usage.QWEN_QUOTA_API,
            "Data": {"cornerstoneParam": {}},
            "V": "1.0",
        },
        separators=(",", ":"),
    )
    query = urlencode(
        {
            "product": route_usage.QWEN_CONSOLE_PRODUCT,
            "action": route_usage.QWEN_CONSOLE_ACTION,
            "api": route_usage.QWEN_QUOTA_API,
        }
    )
    values = {
        "command": "curl",
        "url": f"https://{route_usage.QWEN_CONSOLE_HOST}{route_usage.QWEN_CONSOLE_PATH}?{query}",
        "content_type": "application/x-www-form-urlencoded",
        "cookie": "session=private",
        "body": urlencode(
            {
                "product": route_usage.QWEN_CONSOLE_PRODUCT,
                "action": route_usage.QWEN_CONSOLE_ACTION,
                "sec_token": "private",
                "region": "ap-southeast-1",
                "params": params,
            }
        ),
    }
    values.update(overrides)
    continuation = r"'\'"
    return (
        f"{values['command']} '{values['url']}' {continuation} "
        f"-H 'content-type: {values['content_type']}' {continuation} "
        f"-b '{values['cookie']}' {continuation} --data-raw '{values['body']}'"
    )


def write_qwen_curl(directory: str, **overrides: str) -> Path:
    path = Path(directory) / "qwen.curl"
    path.write_text(qwen_curl_text(**overrides), encoding="utf-8")
    return path


def write_qwen_settings(
    directory: str,
    *,
    model: str = "qwen3.8-max-preview",
    base_url: object = "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1",
    environment_key: object = "PLAN_KEY",
    api_key: object = "secret-key",
    duplicate: bool = False,
) -> Path:
    provider = {
        "id": model,
        "baseUrl": base_url,
        "envKey": environment_key,
    }
    providers = [provider, copy.deepcopy(provider)] if duplicate else [provider]
    path = Path(directory) / "settings.json"
    path.write_text(
        json.dumps(
            {
                "modelProviders": {"ignored": {}, "openai": providers},
                "env": {environment_key: api_key}
                if isinstance(environment_key, str)
                else {},
            }
        ),
        encoding="utf-8",
    )
    return path


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

    def test_parses_and_keys_terminal_local_disabled_models(self) -> None:
        disabled = route_usage.disabled_subagent_models(
            {route_usage.DISABLED_SUBAGENT_MODELS_ENV: " grok-4.5,gpt-5.6-sol,grok-4.5 "}
        )
        self.assertEqual(disabled, frozenset({"gpt-5.6-sol", "grok-4.5"}))
        self.assertNotEqual(
            route_usage.configuration_key(configuration()),
            route_usage.configuration_key(configuration(), disabled),
        )
        with self.assertRaises(ValueError):
            route_usage.disabled_subagent_models(
                {route_usage.DISABLED_SUBAGENT_MODELS_ENV: "model with spaces"}
            )


class RoutingTests(unittest.TestCase):
    def test_grok_worker_avoids_terminal_pipe_deadlocks(self) -> None:
        grok_agent = (
            Path(__file__).parents[3] / "agents" / "claudex-grok.md"
        ).read_text(encoding="utf-8")
        instructions = " ".join(grok_agent.split())
        self.assertIn("Do not pass long heredocs", instructions)
        self.assertIn("dedicated write/edit", instructions)
        self.assertIn("instead of waiting indefinitely", instructions)

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
            routing_hook["timeout"],
            route_usage.USAGE_COMMAND_TIMEOUT_SECONDS
            + 2
            * (
                route_usage.QWEN_REQUEST_TIMEOUT_SECONDS
                + route_usage.QWEN_SUBPROCESS_GRACE_SECONDS
            ),
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

    def test_reports_qwen_quota_and_compatible_only_availability(self) -> None:
        self.assertEqual(
            route_usage.provider_status(report(), "qwen"),
            {
                "available": True,
                "max_used_percent": 5.0,
                "remaining_percent": 95.0,
                "reason": "available-qwen-cloud-quota",
            },
        )
        compatible = qwen_report(None, reason="available-compatible-api-only")
        self.assertIsNone(
            route_usage.provider_status([compatible], "qwen")["remaining_percent"]
        )
        unavailable = route_usage.provider_status(
            [qwen_report(None, available=False)], "qwen"
        )
        self.assertFalse(unavailable["available"])
        self.assertEqual(unavailable["reason"], "usage-unavailable")

    def test_rejects_malformed_explicit_usage(self) -> None:
        malformed = qwen_report(True, available="yes")
        self.assertEqual(
            route_usage.provider_status([malformed], "qwen")["reason"], "unknown"
        )
        for maximum in (True, -1, float("inf"), 101):
            malformed = qwen_report(maximum)
            self.assertEqual(
                route_usage.provider_status([malformed], "qwen")["reason"],
                "unknown",
            )
        self.assertEqual(
            route_usage.explicitly_reported_status({"available": True})["reason"],
            "available",
        )

    def test_selects_all_single_and_fallback_agents(self) -> None:
        self.assertEqual(
            route_usage.routing_summary(report())["selected_agents"],
            ["claudex-qwen", "claudex-gpt", "claudex-grok"],
        )
        self.assertEqual(
            route_usage.routing_summary(report(grok=100))["selected_agents"],
            ["claudex-qwen", "claudex-gpt"],
        )
        unavailable = report(codex=100, grok=100)
        unavailable[-1] = qwen_report(None, available=False)
        fallback = route_usage.routing_summary(unavailable)
        self.assertEqual(fallback["selected_agents"], ["claudex-sonnet"])
        self.assertTrue(fallback["fallback_active"])

    def test_disabled_models_are_excluded_without_deleting_provider_config(self) -> None:
        disabled = frozenset({"gpt-5.6-sol", "grok-4.5"})
        summary = route_usage.routing_summary(report(), configuration(), disabled)
        self.assertEqual(summary["selected_agents"], ["claudex-qwen"])
        self.assertEqual(summary["disabled_subagent_models"], sorted(disabled))
        self.assertEqual(summary["providers"]["codex"]["reason"], "disabled-for-terminal")
        self.assertTrue(summary["providers"]["grok"]["disabled"])

        unavailable = report(codex=100, grok=100)
        unavailable[-1] = qwen_report(None, available=False)
        fallback = route_usage.routing_summary(
            unavailable,
            configuration(),
            frozenset({"claude-sonnet-5"}),
        )
        self.assertEqual(fallback["selected_workers"], [])
        self.assertIsNone(fallback["preferred_worker"])
        self.assertFalse(fallback["fallback_active"])

    def test_failure_fallback_also_honors_terminal_denylist(self) -> None:
        summary = route_usage.fallback_summary(
            "usage-unavailable",
            configuration(),
            frozenset({"claude-sonnet-5"}),
        )
        self.assertEqual(summary["selected_agents"], [])
        self.assertEqual(summary["disabled_subagent_models"], ["claude-sonnet-5"])

    def test_prioritizes_the_provider_with_the_most_known_headroom(self) -> None:
        summary = route_usage.routing_summary(report(codex=80, grok=10, qwen_used=2))
        self.assertEqual(
            summary["selected_agents"],
            ["claudex-qwen", "claudex-grok", "claudex-gpt"],
        )
        self.assertEqual(summary["preferred_worker"]["provider"], "qwen")
        self.assertEqual(summary["providers"]["grok"]["remaining_percent"], 90.0)
        self.assertEqual(summary["providers"]["codex"]["remaining_percent"], 20.0)
        self.assertEqual(summary["providers"]["qwen"]["remaining_percent"], 98.0)

    def test_unknown_qwen_limit_cannot_outrank_known_capacity(self) -> None:
        unknown = report(codex=99, grok=100)
        unknown[-1] = qwen_report(None, reason="available-compatible-api-only")
        summary = route_usage.routing_summary(unknown)
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
        self.assertIn("absolute SubAgent denylist", context)
        self.assertIn("even when the user names it", context)
        self.assertIn("complete tool set and permission context", context)
        self.assertIn("never add an implicit read-only", context)
        self.assertIn("background execution would auto-deny", context)
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
    def test_converts_qwen_quota_windows_to_percentages(self) -> None:
        entry = route_usage.qwen_quota_entry(quota_payload(), "qwen")
        self.assertEqual(entry["maxUsedPercent"], 2.0)
        self.assertEqual(entry["quotaWindows"][0]["remainingPercent"], 99.0)
        self.assertEqual(
            entry["quotaWindows"][1]["resetAtMilliseconds"], 1_786_000_000_000
        )
        exhausted = route_usage.qwen_quota_entry(quota_payload(seven_day=1), "qwen")
        self.assertFalse(exhausted["available"])
        self.assertEqual(exhausted["reason"], "exhausted")

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
    def test_runs_validated_qwen_quota_curl_without_a_shell(
        self, run: mock.Mock
    ) -> None:
        run.return_value = subprocess.CompletedProcess(
            [], 0, json.dumps(quota_payload()), ""
        )
        with tempfile.TemporaryDirectory() as directory:
            entry = route_usage.run_qwen_quota(
                "curl-test", write_qwen_curl(directory), "qwen"
            )
        self.assertEqual(entry["maxUsedPercent"], 2.0)
        arguments = run.call_args.args[0]
        self.assertEqual(arguments[0], "curl-test")
        self.assertNotIn("shell", run.call_args.kwargs)
        self.assertIn("--fail-with-body", arguments)
        self.assertIn("session=private", arguments)
        self.assertEqual(
            run.call_args.kwargs["timeout"],
            route_usage.QWEN_REQUEST_TIMEOUT_SECONDS
            + route_usage.QWEN_SUBPROCESS_GRACE_SECONDS,
        )

    def test_rejects_malformed_qwen_quota_responses(self) -> None:
        invalid_payloads = [None, {}, {"data": {"DataV2": {"data": {"data": []}}}}]
        for payload in invalid_payloads:
            with self.assertRaises(ValueError):
                route_usage.qwen_quota_entry(payload, "qwen")
        for value in (True, -1, float("inf"), 1.1):
            with self.assertRaises(ValueError):
                route_usage.qwen_quota_entry(quota_payload(five_hour=value), "qwen")
        for value in (True, -1, float("inf"), 1.5, "bad"):
            with self.assertRaises(ValueError):
                route_usage.qwen_quota_entry(
                    quota_payload(five_hour_reset=value), "qwen"
                )

    @mock.patch("route_usage.qwen_usage_entry", return_value=qwen_report())
    @mock.patch("route_usage.run_codexbar", return_value=report()[:2])
    def test_collects_codexbar_and_qwen_usage(
        self, _codexbar: mock.Mock, qwen: mock.Mock
    ) -> None:
        collected = route_usage.collect_usage(
            configuration(), "codexbar", "curl", {"HOME": "/test-home"}, 100
        )
        self.assertEqual(
            [entry["provider"] for entry in collected], ["codex", "grok", "qwen"]
        )
        qwen.assert_called_once_with(
            "curl",
            "qwen",
            "qwen3.8-max-preview",
            route_usage.DEFAULT_QWEN_CURL,
            Path("/test-home/.qwen/settings.json"),
            Path("/test-home/.cache/claudex/qwen-quota.json"),
            100,
        )

    @mock.patch("route_usage.qwen_usage_entry", return_value=qwen_report())
    @mock.patch(
        "route_usage.run_codexbar",
        return_value=[*report()[:2], {"provider": "qwen", "usage": {}}],
    )
    def test_qwen_cli_usage_replaces_a_codexbar_qwen_entry(
        self, _codexbar: mock.Mock, _qwen: mock.Mock
    ) -> None:
        collected = route_usage.collect_usage(configuration(), "codexbar", "curl")
        qwen_entries = [entry for entry in collected if entry.get("provider") == "qwen"]
        self.assertEqual(qwen_entries, [qwen_report()])
        qwen_worker = next(
            worker
            for worker in route_usage.routing_summary(collected)["selected_workers"]
            if worker["provider"] == "qwen"
        )
        self.assertEqual(qwen_worker["model"], "qwen3.8-max-preview")
        self.assertEqual(qwen_worker["model_prefixes"], ["qwen"])

    @mock.patch(
        "route_usage.qwen_usage_entry",
        return_value=qwen_report(None, available=False),
    )
    @mock.patch("route_usage.run_codexbar", return_value=report()[:2])
    def test_qwen_usage_failure_does_not_disable_codexbar_providers(
        self, _codexbar: mock.Mock, _qwen: mock.Mock
    ) -> None:
        collected = route_usage.collect_usage(configuration(), "codexbar", "curl")
        summary = route_usage.routing_summary(collected)
        self.assertEqual(summary["selected_agents"], ["claudex-gpt", "claudex-grok"])
        self.assertEqual(summary["providers"]["qwen"]["reason"], "usage-unavailable")

    @mock.patch("route_usage.qwen_usage_entry", return_value=qwen_report())
    @mock.patch("route_usage.run_codexbar", side_effect=OSError("missing"))
    def test_codexbar_failure_does_not_disable_qwen(
        self, _codexbar: mock.Mock, _qwen: mock.Mock
    ) -> None:
        collected = route_usage.collect_usage(configuration(), "codexbar", "curl")
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
                    "--curl-program",
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


class QwenCurlTests(unittest.TestCase):
    def test_parses_browser_curl_and_inline_supported_options(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = write_qwen_curl(directory)
            request = route_usage.qwen_curl_request(path)
            self.assertEqual(request["cookie"], "session=private")
            self.assertIn("sec_token=private", request["body"])
            path.write_text(qwen_curl_text().replace(r"'\'", "\\\n"), encoding="utf-8")
            self.assertEqual(
                route_usage.qwen_curl_request(path)["cookie"], "session=private"
            )
            inline = (
                qwen_curl_text()
                .replace(
                    "-H 'content-type: application/x-www-form-urlencoded'",
                    "--header='content-type: application/x-www-form-urlencoded'",
                )
                .replace("-b 'session=private'", "--cookie='session=private'")
                .replace("--data-raw '", "--data='")
            )
            path.write_text(inline, encoding="utf-8")
            self.assertEqual(
                route_usage.qwen_curl_request(path)["cookie"], "session=private"
            )

    def test_requires_curl_one_url_and_supported_complete_options(self) -> None:
        cases = [
            "",
            qwen_curl_text(command="wget"),
            qwen_curl_text() + " 'https://example.com'",
            qwen_curl_text() + " --location",
            "curl --header",
        ]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "curl.txt"
            for value in cases:
                path.write_text(value, encoding="utf-8")
                with self.assertRaises(ValueError):
                    route_usage.qwen_curl_request(path)

    def test_rejects_duplicate_or_empty_sensitive_values(self) -> None:
        cases = [
            qwen_curl_text() + " -b 'other=value'",
            qwen_curl_text() + " --data 'other=value'",
            qwen_curl_text(cookie=""),
            qwen_curl_text(body=""),
        ]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "curl.txt"
            for value in cases:
                path.write_text(value, encoding="utf-8")
                with self.assertRaises(ValueError):
                    route_usage.qwen_curl_request(path)

    def test_rejects_unexpected_endpoint_components(self) -> None:
        valid = qwen_curl_text()
        replacements = [
            ("https://", "http://"),
            (route_usage.QWEN_CONSOLE_HOST, "example.com"),
            (route_usage.QWEN_CONSOLE_HOST, f"{route_usage.QWEN_CONSOLE_HOST}:8443"),
            (route_usage.QWEN_CONSOLE_PATH, "/other"),
            ("https://", "https://user@example.invalid@"),
            ("&api=", "#fragment&api="),
        ]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "curl.txt"
            for before, after in replacements:
                path.write_text(valid.replace(before, after, 1), encoding="utf-8")
                with self.assertRaises(ValueError):
                    route_usage.qwen_curl_request(path)

    def test_rejects_unexpected_query_fields_and_values(self) -> None:
        text = qwen_curl_text()
        cases = [
            text.replace("?product=", "?extra=x&product="),
            text.replace("product=sfm_bailian", "product=wrong", 1),
            text.replace("action=IntlBroadScopeAspnGateway", "action=wrong", 1),
            text.replace("api=zeldaHttp", "api=wrong", 1),
        ]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "curl.txt"
            for value in cases:
                path.write_text(value, encoding="utf-8")
                with self.assertRaises(ValueError):
                    route_usage.qwen_curl_request(path)

    def test_rejects_unexpected_form_fields_and_values(self) -> None:
        text = qwen_curl_text()
        body = route_usage.qwen_curl_request(self.write_text_to_temporary_file(text))[
            "body"
        ]
        form = {key: values[0] for key, values in route_usage.parse_qs(body).items()}
        cases: list[dict[str, str]] = []
        for key, value in [
            ("product", "wrong"),
            ("action", "wrong"),
            ("sec_token", ""),
            ("region", "wrong"),
        ]:
            changed = dict(form)
            changed[key] = value
            cases.append(changed)
        changed = dict(form)
        changed["extra"] = "wrong"
        cases.append(changed)
        for parameters in [
            [],
            {"Api": "wrong", "Data": {}, "V": "1.0"},
            {"Api": route_usage.QWEN_QUOTA_API, "Data": [], "V": "1.0"},
            {"Api": route_usage.QWEN_QUOTA_API, "Data": {}, "V": "wrong"},
        ]:
            changed = dict(form)
            changed["params"] = json.dumps(parameters)
            cases.append(changed)
        changed = dict(form)
        changed["params"] = "not-json"
        cases.append(changed)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "curl.txt"
            for changed in cases:
                path.write_text(
                    qwen_curl_text(body=urlencode(changed)), encoding="utf-8"
                )
                with self.assertRaises((ValueError, json.JSONDecodeError)):
                    route_usage.qwen_curl_request(path)
            for overrides in [
                {"cookie": "no-cookie-pair"},
                {"cookie": "session=private\ninjected=true"},
                {"content_type": "application/json"},
            ]:
                path.write_text(qwen_curl_text(**overrides), encoding="utf-8")
                with self.assertRaises(ValueError):
                    route_usage.qwen_curl_request(path)

    def test_single_value_rejects_missing_empty_and_duplicate_values(self) -> None:
        for values in ({}, {"key": [""]}, {"key": ["one", "two"]}):
            with self.assertRaises(ValueError):
                route_usage.single_value(values, "key")

    def write_text_to_temporary_file(self, value: str) -> Path:
        temporary = tempfile.NamedTemporaryFile("w", delete=False, encoding="utf-8")
        self.addCleanup(Path(temporary.name).unlink, missing_ok=True)
        with temporary:
            temporary.write(value)
        return Path(temporary.name)


class QwenCacheTests(unittest.TestCase):
    def test_quota_cache_is_private_fresh_then_stale_at_one_hour(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "quota.json"
            entry = route_usage.qwen_quota_entry(quota_payload(), "qwen")
            route_usage.write_qwen_quota_cache(path, entry, 100)
            self.assertEqual(
                json.loads(path.read_text(encoding="utf-8"))["fetched_at"],
                "1970-01-01T00:01:40.000000Z",
            )
            self.assertEqual(route_usage.qwen_quota_cache_entry(path, 100), entry)
            self.assertEqual(route_usage.qwen_quota_cache_entry(path, 3_699), entry)
            self.assertIsNone(route_usage.qwen_quota_cache_entry(path, 3_700))
            self.assertIsNone(route_usage.qwen_quota_cache_entry(path, 99))
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            contents = path.read_text(encoding="utf-8")
            self.assertNotIn("session=private", contents)
            self.assertNotIn("secret-key", contents)

    def test_formats_and_validates_utc_acquisition_datetime(self) -> None:
        value = route_usage.format_utc_datetime(1_784_937_600.25)
        self.assertTrue(value.endswith("Z"))
        self.assertEqual(route_usage.parse_utc_datetime(value), 1_784_937_600.25)
        for invalid in (None, 1_784_937_600, "2026-07-25T00:00:00", "invalidZ"):
            with self.assertRaises((TypeError, ValueError)):
                route_usage.parse_utc_datetime(invalid)

    def test_quota_cache_rejects_missing_malformed_and_invalid_entries(self) -> None:
        invalid: list[object] = [
            "not-json",
            {},
            {"fetched_at": "bad", "entry": {}},
            {"fetched_at": 1, "entry": qwen_report()},
            {"fetched_at": 1, "entry": []},
            {"fetched_at": 1, "entry": qwen_report(None)},
            {
                "fetched_at": 1,
                "entry": {**qwen_report(), "provider": "other"},
            },
            {
                "fetched_at": 1,
                "entry": {**qwen_report(), "maxUsedPercent": True},
            },
        ]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "quota.json"
            self.assertIsNone(route_usage.qwen_quota_cache_entry(path, 1))
            for value in invalid:
                path.write_text(
                    value if isinstance(value, str) else json.dumps(value),
                    encoding="utf-8",
                )
                self.assertIsNone(route_usage.qwen_quota_cache_entry(path, 1))

    def test_quota_cache_path_honors_the_effective_home(self) -> None:
        self.assertEqual(
            route_usage.qwen_quota_cache_path({"HOME": "/effective-home"}),
            Path("/effective-home/.cache/claudex/qwen-quota.json"),
        )

    def test_stale_quota_invalidates_only_quota_based_routing_cache(self) -> None:
        config = configuration()
        summary = route_usage.routing_summary(report(), config)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "quota.json"
            entry = route_usage.qwen_quota_entry(quota_payload(), "qwen")
            route_usage.write_qwen_quota_cache(path, entry, 100)
            self.assertFalse(
                route_usage.qwen_quota_refresh_due(summary, config, path, 3_699)
            )
            self.assertTrue(
                route_usage.qwen_quota_refresh_due(summary, config, path, 3_700)
            )
            summary["providers"]["qwen"]["reason"] = "available-compatible-api-only"
            self.assertFalse(
                route_usage.qwen_quota_refresh_due(summary, config, path, 3_700)
            )
            self.assertFalse(
                route_usage.qwen_quota_refresh_due([], config, path, 3_700)
            )
            self.assertFalse(
                route_usage.qwen_quota_refresh_due(
                    {"providers": []}, config, path, 3_700
                )
            )


class QwenCompatibleTests(unittest.TestCase):
    def test_reads_existing_qwen_compatible_configuration(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            endpoint, key = route_usage.qwen_compatible_configuration(
                write_qwen_settings(directory), "qwen3.8-max-preview"
            )
        self.assertEqual(
            endpoint,
            "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1/models",
        )
        self.assertEqual(key, "secret-key")

    def test_rejects_invalid_qwen_settings_structure_and_model_count(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "settings.json"
            for value in [
                {},
                {"modelProviders": {}, "env": []},
            ]:
                path.write_text(json.dumps(value), encoding="utf-8")
                with self.assertRaises(ValueError):
                    route_usage.qwen_compatible_configuration(path, "model")
            with self.assertRaises(ValueError):
                route_usage.qwen_compatible_configuration(
                    write_qwen_settings(directory), "missing"
                )
            with self.assertRaises(ValueError):
                route_usage.qwen_compatible_configuration(
                    write_qwen_settings(directory, duplicate=True),
                    "qwen3.8-max-preview",
                )

    def test_rejects_missing_credentials_and_unexpected_endpoints(self) -> None:
        invalid_settings = [
            {"base_url": None},
            {"environment_key": None},
            {"api_key": ""},
        ]
        invalid_urls = [
            "http://token-plan.region.maas.aliyuncs.com/compatible-mode/v1",
            "https:///compatible-mode/v1",
            "https://token-plan.region.example.com/compatible-mode/v1",
            "https://other.region.maas.aliyuncs.com/compatible-mode/v1",
            "https://token-plan.region.maas.aliyuncs.com/other",
            "https://token-plan.region.maas.aliyuncs.com:8443/compatible-mode/v1",
            "https://token-plan.region.maas.aliyuncs.com/compatible-mode/v1;param",
            "https://token-plan.region.maas.aliyuncs.com/compatible-mode/v1?query=x",
            "https://token-plan.region.maas.aliyuncs.com/compatible-mode/v1#fragment",
            "https://user@token-plan.region.maas.aliyuncs.com/compatible-mode/v1",
        ]
        with tempfile.TemporaryDirectory() as directory:
            for values in invalid_settings:
                with self.assertRaises(ValueError):
                    route_usage.qwen_compatible_configuration(
                        write_qwen_settings(directory, **values),
                        "qwen3.8-max-preview",
                    )
            for base_url in invalid_urls:
                with self.assertRaises(ValueError):
                    route_usage.qwen_compatible_configuration(
                        write_qwen_settings(directory, base_url=base_url),
                        "qwen3.8-max-preview",
                    )

    @mock.patch("route_usage.subprocess.run")
    def test_verifies_compatible_models_endpoint_without_generation(
        self, run: mock.Mock
    ) -> None:
        run.return_value = subprocess.CompletedProcess([], 0, "", "")
        with tempfile.TemporaryDirectory() as directory:
            self.assertTrue(
                route_usage.qwen_compatible_available(
                    "curl-test",
                    write_qwen_settings(directory),
                    "qwen3.8-max-preview",
                )
            )
        arguments = run.call_args.args[0]
        self.assertIn("--output", arguments)
        self.assertIn(os.devnull, arguments)
        self.assertTrue(arguments[-1].endswith("/models"))
        self.assertFalse(any("chat/completions" in value for value in arguments))


class QwenFallbackTests(unittest.TestCase):
    @mock.patch("route_usage.run_qwen_quota")
    def test_reuses_fresh_quota_without_network(self, quota: mock.Mock) -> None:
        with tempfile.TemporaryDirectory() as directory:
            cache = Path(directory) / "quota.json"
            entry = route_usage.qwen_quota_entry(quota_payload(), "qwen")
            route_usage.write_qwen_quota_cache(cache, entry, 100)
            actual = route_usage.qwen_usage_entry(
                "curl", "qwen", "model", Path("curl"), Path("settings"), cache, 101
            )
        self.assertEqual(actual, entry)
        quota.assert_not_called()

    @mock.patch("route_usage.write_qwen_quota_cache")
    @mock.patch("route_usage.run_qwen_quota")
    def test_refreshes_and_caches_stale_quota(
        self, quota: mock.Mock, write: mock.Mock
    ) -> None:
        entry = route_usage.qwen_quota_entry(quota_payload(), "qwen")
        quota.return_value = entry
        actual = route_usage.qwen_usage_entry(
            "curl", "qwen", "model", Path("curl"), Path("settings"), Path("cache"), 5
        )
        self.assertEqual(actual, entry)
        write.assert_called_once_with(Path("cache"), entry, 5)

    @mock.patch("route_usage.qwen_compatible_available", return_value=True)
    @mock.patch("route_usage.run_qwen_quota", side_effect=OSError("expired"))
    def test_refresh_failure_falls_back_to_compatible_api(
        self, _quota: mock.Mock, compatible: mock.Mock
    ) -> None:
        actual = route_usage.qwen_usage_entry(
            "curl", "qwen", "model", Path("curl"), Path("settings"), Path("cache"), 5
        )
        self.assertEqual(
            actual, qwen_report(None, reason="available-compatible-api-only")
        )
        compatible.assert_called_once_with("curl", Path("settings"), "model")

    @mock.patch(
        "route_usage.qwen_compatible_available",
        side_effect=subprocess.SubprocessError("unavailable"),
    )
    @mock.patch("route_usage.run_qwen_quota", side_effect=ValueError("expired"))
    def test_both_qwen_sources_can_fail_without_raising(
        self, _quota: mock.Mock, _compatible: mock.Mock
    ) -> None:
        actual = route_usage.qwen_usage_entry(
            "curl", "qwen", "model", Path("curl"), Path("settings"), Path("cache"), 5
        )
        self.assertEqual(actual, qwen_report(None, available=False))


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
                "--curl-program",
                "curl-tool",
            ],
        ):
            arguments = route_usage.parse_arguments()
        self.assertEqual(arguments.input, Path("usage.json"))
        self.assertEqual(arguments.config, Path("providers.json"))
        self.assertTrue(arguments.no_cache)
        self.assertEqual(arguments.codexbar_program, "usage-tool")
        self.assertEqual(arguments.curl_program, "curl-tool")

    @mock.patch("route_usage.qwen_quota_refresh_due", return_value=False)
    @mock.patch("route_usage.collect_usage")
    @mock.patch("route_usage.read_cache")
    def test_main_reuses_a_fresh_cache(
        self,
        read_cache: mock.Mock,
        collect_usage: mock.Mock,
        _quota_due: mock.Mock,
    ) -> None:
        read_cache.return_value = route_usage.routing_summary(report())
        output = self.run_main()
        self.assertIn("claudex-gpt", output)
        collect_usage.assert_not_called()

    @mock.patch("route_usage.write_cache")
    @mock.patch("route_usage.collect_usage", return_value=report())
    @mock.patch("route_usage.qwen_quota_refresh_due", return_value=True)
    @mock.patch("route_usage.read_cache")
    def test_main_refreshes_routing_when_its_qwen_quota_expires(
        self,
        read_cache: mock.Mock,
        _quota_due: mock.Mock,
        collect_usage: mock.Mock,
        write_cache: mock.Mock,
    ) -> None:
        read_cache.return_value = route_usage.routing_summary(report())
        output = self.run_main()
        self.assertIn("claudex-qwen", output)
        collect_usage.assert_called_once()
        write_cache.assert_called_once()

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
        self.assertIn('"selected_agents":["claudex-qwen","claudex-gpt"]', context)
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

    @mock.patch("route_usage.collect_usage", return_value=report())
    @mock.patch("route_usage.read_cache", return_value=None)
    def test_main_applies_each_terminal_environment_to_selection_and_cache_key(
        self, read_cache: mock.Mock, _collect_usage: mock.Mock
    ) -> None:
        with mock.patch.dict(
            os.environ,
            {route_usage.DISABLED_SUBAGENT_MODELS_ENV: "gpt-5.6-sol,grok-4.5"},
        ):
            output = self.run_main("--no-cache")
        context = json.loads(output)["hookSpecificOutput"]["additionalContext"]
        self.assertIn('"selected_agents":["claudex-qwen"]', context)
        expected_key = route_usage.configuration_key(
            configuration(), frozenset({"gpt-5.6-sol", "grok-4.5"})
        )
        self.assertEqual(read_cache.call_args.args[3], expected_key)

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
