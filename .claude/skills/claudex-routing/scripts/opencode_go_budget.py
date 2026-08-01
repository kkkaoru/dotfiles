#!/usr/bin/env python3
"""Evaluate the published OpenCode Go request budget for one usage window.

OpenCode Go publishes an *estimated* request allowance for each model and
window.  CodexBar exposes that provider's current window as ``usedPercent``;
it does not expose a request counter.  This module keeps the two facts
separate and reports an estimate instead of pretending that a local counter
is authoritative.
"""

from __future__ import annotations

import math
from typing import Any, Mapping

DEFAULT_PROVIDER = "opencode-go"
DEFAULT_MODEL = "opencode-go/deepseek-v4-flash"
DEFAULT_USAGE_PROVIDER = "opencodego"
DEFAULT_USAGE_WINDOW = "primary"
DEFAULT_WINDOW_MINUTES = 5 * 60
DEFAULT_ESTIMATED_REQUESTS = 31_650


def valid_request_budget(value: Any) -> bool:
    """Validate the config object without accepting implicit or unsafe values."""
    if not isinstance(value, Mapping):
        return False
    if set(value) != {"estimatedRequests", "windowMinutes", "usageWindow"}:
        return False
    requests = value.get("estimatedRequests")
    minutes = value.get("windowMinutes")
    window = value.get("usageWindow")
    return (
        isinstance(requests, int)
        and not isinstance(requests, bool)
        and requests > 0
        and isinstance(minutes, int)
        and not isinstance(minutes, bool)
        and minutes > 0
        and isinstance(window, str)
        and bool(window)
        and window.isascii()
        and all("!" <= char <= "~" for char in window)
    )


def normalized_request_budget(value: Any) -> dict[str, Any] | None:
    """Return a copy of a valid budget config, or ``None`` for no budget."""
    if not valid_request_budget(value):
        return None
    # Mapping values are copied so callers cannot mutate provider config via
    # the sanitized routing result.
    return {
        "estimatedRequests": int(value["estimatedRequests"]),
        "windowMinutes": int(value["windowMinutes"]),
        "usageWindow": str(value["usageWindow"]),
    }


def _status(
    available: bool,
    used_percent: float | None,
    reason: str,
    budget: Mapping[str, Any],
    **details: Any,
) -> dict[str, Any]:
    configured = normalized_request_budget(budget)
    if configured is None:
        raise ValueError("invalid OpenCode Go request budget")
    result: dict[str, Any] = {
        "available": available,
        "max_used_percent": used_percent,
        "remaining_percent": (
            None if used_percent is None else max(0.0, 100.0 - used_percent)
        ),
        "reason": reason,
        "request_budget": {
            "estimated_requests": configured["estimatedRequests"],
            "window_minutes": configured["windowMinutes"],
            "usage_window": configured["usageWindow"],
            "known": used_percent is not None,
            "used_percent": used_percent,
            **details,
        },
    }
    return result


def _valid_percent(value: Any) -> bool:
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and math.isfinite(float(value))
        and 0.0 <= float(value) <= 100.0
    )


def _entry(report: Any, provider: str) -> Mapping[str, Any] | None:
    if not isinstance(report, list):
        return None
    wanted = provider.casefold()
    for item in report:
        if (
            isinstance(item, Mapping)
            and str(item.get("provider", "")).casefold() == wanted
        ):
            return item
    return None


def evaluate(
    report: Any,
    usage_provider: str = DEFAULT_USAGE_PROVIDER,
    budget: Mapping[str, Any] | None = None,
) -> dict[str, Any] | None:
    """Evaluate a configured OpenCode Go budget against a CodexBar report.

    ``None`` means the provider has no request-budget configuration and the
    caller should use its normal quota handling.  A configured budget is
    considered unavailable when the provider/window/counter cannot be
    validated; routing must not silently fall back to another usage window.
    """
    normalized = normalized_request_budget(budget)
    if normalized is None:
        if budget is None:
            return None
        raise ValueError("invalid OpenCode Go request budget")

    entry = _entry(report, usage_provider)
    if entry is None:
        return _status(False, None, "missing", normalized)
    usage = entry.get("usage")
    window_name = normalized["usageWindow"]
    window = usage.get(window_name) if isinstance(usage, Mapping) else None
    if not isinstance(window, Mapping):
        return _status(False, None, "request-budget-window-missing", normalized)

    used_percent = window.get("usedPercent")
    reported_minutes = window.get("windowMinutes")
    if not _valid_percent(used_percent):
        return _status(False, None, "request-budget-usage-unknown", normalized)
    if (
        not isinstance(reported_minutes, int)
        or isinstance(reported_minutes, bool)
        or reported_minutes != normalized["windowMinutes"]
    ):
        return _status(
            False,
            None,
            "request-budget-window-mismatch",
            normalized,
            reported_window_minutes=reported_minutes,
        )

    percent = float(used_percent)
    total = float(normalized["estimatedRequests"])
    estimated_used = round(total * percent / 100.0, 3)
    estimated_remaining = round(max(0.0, total - estimated_used), 3)
    reset_at = window.get("resetsAt")
    if not isinstance(reset_at, str) or not reset_at:
        reset_at = None
    return _status(
        percent < 100.0,
        percent,
        "available" if percent < 100.0 else "request-budget-exhausted",
        normalized,
        reported_window_minutes=reported_minutes,
        estimated_used_requests=estimated_used,
        estimated_remaining_requests=estimated_remaining,
        resets_at=reset_at,
    )


def provider_budget(provider: Mapping[str, Any]) -> dict[str, Any] | None:
    """Read and validate the provider's optional request-budget policy."""
    value = provider.get("requestBudget")
    return normalized_request_budget(value) if value is not None else None


__all__ = [
    "DEFAULT_ESTIMATED_REQUESTS",
    "DEFAULT_MODEL",
    "DEFAULT_PROVIDER",
    "DEFAULT_USAGE_PROVIDER",
    "DEFAULT_USAGE_WINDOW",
    "DEFAULT_WINDOW_MINUTES",
    "evaluate",
    "normalized_request_budget",
    "provider_budget",
    "valid_request_budget",
]
