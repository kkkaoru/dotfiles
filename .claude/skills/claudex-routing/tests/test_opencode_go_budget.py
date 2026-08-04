from __future__ import annotations

import copy
import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parents[1] / "scripts"))

import opencode_go_budget


FLASH_BUDGET = {
    "estimatedRequests": 31650,
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
        self.assertTrue(opencode_go_budget.valid_request_budget(FLASH_BUDGET))
        for invalid in [
            None,
            {},
            {**FLASH_BUDGET, "estimatedRequests": 0},
            {**FLASH_BUDGET, "estimatedRequests": True},
            {**FLASH_BUDGET, "windowMinutes": -1},
            {**FLASH_BUDGET, "windowMinutes": 300.0},
            {**FLASH_BUDGET, "usageWindow": "primary window"},
            {**FLASH_BUDGET, "extra": "not allowed"},
        ]:
            with self.subTest(invalid=invalid):
                self.assertFalse(opencode_go_budget.valid_request_budget(invalid))

    def test_converts_codexbar_percentage_to_a_five_hour_request_estimate(self) -> None:
        actual = opencode_go_budget.evaluate(report(), budget=FLASH_BUDGET)
        self.assertIsNotNone(actual)
        assert actual is not None
        self.assertTrue(actual["available"])
        self.assertEqual(actual["max_used_percent"], 1.2)
        self.assertEqual(actual["remaining_percent"], 98.8)
        self.assertEqual(actual["reason"], "available")
        self.assertEqual(
            actual["request_budget"],
            {
                "estimated_requests": 31650,
                "window_minutes": 300,
                "usage_window": "primary",
                "known": True,
                "reported_window_minutes": 300,
                "used_percent": 1.2,
                "estimated_used_requests": 379.8,
                "estimated_remaining_requests": 31270.2,
                "resets_at": "2026-07-31T00:39:13Z",
            },
        )

    def test_uses_the_configured_window_not_the_weekly_percentage(self) -> None:
        actual = opencode_go_budget.evaluate(report(100.0), budget=FLASH_BUDGET)
        self.assertIsNotNone(actual)
        assert actual is not None
        self.assertFalse(actual["available"])
        self.assertEqual(actual["reason"], "request-budget-exhausted")
        self.assertEqual(actual["request_budget"]["estimated_used_requests"], 31650.0)

    def test_requires_an_authoritative_five_hour_window(self) -> None:
        for malformed in [
            report(used_percent="unknown"),
            report(window_minutes=10080),
            [{"provider": "opencodego", "usage": {"secondary": {"usedPercent": 1}}}],
            [],
        ]:
            with self.subTest(malformed=malformed):
                actual = opencode_go_budget.evaluate(malformed, budget=FLASH_BUDGET)
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
        provider = {"requestBudget": copy.deepcopy(FLASH_BUDGET)}
        actual = opencode_go_budget.provider_budget(provider)
        self.assertEqual(actual, FLASH_BUDGET)
        assert actual is not None
        actual["estimatedRequests"] = 1
        self.assertEqual(provider["requestBudget"]["estimatedRequests"], 31650)

    def test_serialized_policy_is_json_safe(self) -> None:
        self.assertEqual(json.loads(json.dumps(FLASH_BUDGET)), FLASH_BUDGET)

    def test_non_list_report_is_missing(self) -> None:
        actual = opencode_go_budget.evaluate("not-a-report", budget=FLASH_BUDGET)
        self.assertIsNotNone(actual)
        assert actual is not None
        self.assertFalse(actual["available"])
        self.assertEqual(actual["reason"], "missing")
        self.assertFalse(actual["request_budget"]["known"])

    def test_invalid_non_none_budget_is_rejected(self) -> None:
        with self.assertRaises(ValueError):
            opencode_go_budget.evaluate(report(), budget={"estimatedRequests": 0})

    def test_finds_provider_by_casefolded_name_in_any_position(self) -> None:
        multi = [
            {"provider": "unrelated", "usage": {}},
            {
                "provider": "OpenCodeGo",
                "usage": {"primary": {"usedPercent": 50.0, "windowMinutes": 300}},
            },
        ]
        actual = opencode_go_budget.evaluate(multi, budget=FLASH_BUDGET)
        self.assertIsNotNone(actual)
        assert actual is not None
        self.assertEqual(actual["max_used_percent"], 50.0)
        self.assertEqual(actual["reason"], "available")

    def test_missing_resets_at_is_optional(self) -> None:
        actual = opencode_go_budget.evaluate(report(resets_at=None), budget=FLASH_BUDGET)
        self.assertIsNotNone(actual)
        assert actual is not None
        self.assertEqual(actual["reason"], "available")
        self.assertIsNone(actual["request_budget"]["resets_at"])

    def test_status_rejects_an_invalid_budget(self) -> None:
        with self.assertRaises(ValueError):
            opencode_go_budget._status(
                True, 1.0, "available", {"estimatedRequests": 0}
            )


if __name__ == "__main__":
    unittest.main()
