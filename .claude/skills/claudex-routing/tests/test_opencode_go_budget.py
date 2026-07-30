from __future__ import annotations

import copy
import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parents[1] / "scripts"))

import opencode_go_budget


PRO_BUDGET = {
    "estimatedRequests": 3450,
    "windowMinutes": 300,
    "usageWindow": "primary",
}


def report(
    used_percent: object = 1.2,
    window_minutes: object = 300,
    resets_at: object = "2026-07-31T00:39:13Z",
) -> list[dict[str, object]]:
    return [
        {
            "provider": "opencodego",
            "usage": {
                "primary": {
                    "usedPercent": used_percent,
                    "windowMinutes": window_minutes,
                    "resetsAt": resets_at,
                },
                "secondary": {"usedPercent": 99.0, "windowMinutes": 10080},
            },
        }
    ]


class OpenCodeGoBudgetTests(unittest.TestCase):
    def test_validates_the_published_budget_schema(self) -> None:
        self.assertTrue(opencode_go_budget.valid_request_budget(PRO_BUDGET))
        for invalid in [
            None,
            {},
            {**PRO_BUDGET, "estimatedRequests": 0},
            {**PRO_BUDGET, "estimatedRequests": True},
            {**PRO_BUDGET, "windowMinutes": -1},
            {**PRO_BUDGET, "windowMinutes": 300.0},
            {**PRO_BUDGET, "usageWindow": "primary window"},
            {**PRO_BUDGET, "extra": "not allowed"},
        ]:
            with self.subTest(invalid=invalid):
                self.assertFalse(opencode_go_budget.valid_request_budget(invalid))

    def test_converts_codexbar_percentage_to_a_five_hour_request_estimate(self) -> None:
        actual = opencode_go_budget.evaluate(report(), budget=PRO_BUDGET)
        self.assertIsNotNone(actual)
        assert actual is not None
        self.assertTrue(actual["available"])
        self.assertEqual(actual["max_used_percent"], 1.2)
        self.assertEqual(actual["remaining_percent"], 98.8)
        self.assertEqual(actual["reason"], "available")
        self.assertEqual(
            actual["request_budget"],
            {
                "estimated_requests": 3450,
                "window_minutes": 300,
                "usage_window": "primary",
                "known": True,
                "reported_window_minutes": 300,
                "used_percent": 1.2,
                "estimated_used_requests": 41.4,
                "estimated_remaining_requests": 3408.6,
                "resets_at": "2026-07-31T00:39:13Z",
            },
        )

    def test_uses_the_configured_window_not_the_weekly_percentage(self) -> None:
        actual = opencode_go_budget.evaluate(report(100.0), budget=PRO_BUDGET)
        self.assertIsNotNone(actual)
        assert actual is not None
        self.assertFalse(actual["available"])
        self.assertEqual(actual["reason"], "request-budget-exhausted")
        self.assertEqual(actual["request_budget"]["estimated_used_requests"], 3450.0)

    def test_requires_an_authoritative_five_hour_window(self) -> None:
        for malformed in [
            report(used_percent="unknown"),
            report(window_minutes=10080),
            [{"provider": "opencodego", "usage": {"secondary": {"usedPercent": 1}}}],
            [],
        ]:
            with self.subTest(malformed=malformed):
                actual = opencode_go_budget.evaluate(malformed, budget=PRO_BUDGET)
                self.assertIsNotNone(actual)
                assert actual is not None
                self.assertFalse(actual["available"])
                self.assertIsNone(actual["max_used_percent"])
                self.assertTrue(
                    actual["reason"] == "missing"
                    or actual["reason"].startswith("request-budget-")
                )
                self.assertFalse(actual["request_budget"]["known"])

    def test_missing_budget_keeps_normal_routing_path_explicit(self) -> None:
        self.assertIsNone(opencode_go_budget.evaluate(report(), budget=None))

    def test_provider_budget_returns_a_copy(self) -> None:
        provider = {"requestBudget": copy.deepcopy(PRO_BUDGET)}
        actual = opencode_go_budget.provider_budget(provider)
        self.assertEqual(actual, PRO_BUDGET)
        assert actual is not None
        actual["estimatedRequests"] = 1
        self.assertEqual(provider["requestBudget"]["estimatedRequests"], 3450)

    def test_serialized_policy_is_json_safe(self) -> None:
        self.assertEqual(json.loads(json.dumps(PRO_BUDGET)), PRO_BUDGET)


if __name__ == "__main__":
    unittest.main()
