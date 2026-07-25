#!/usr/bin/env python3
"""Emit sanitized routing context from Codexbar and provider CLI usage."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any

DEFAULT_CACHE_SECONDS = 300
REPOSITORY_CONFIG = Path(__file__).parents[4] / ".config/claudex/providers.json"
QWEN_USAGE_PROVIDER = "qwen"
QWEN_USAGE_FILENAME = "usage.json"
USAGE_COMMAND_TIMEOUT_SECONDS = 45


def config_path(environment: dict[str, str], requested: Path | None = None) -> Path:
    """Resolve an explicit, installed, or repository-local provider config."""
    if requested:
        return requested
    if configured := environment.get("CLAUDEX_PROVIDER_CONFIG"):
        return Path(configured).expanduser()
    installed = Path.home() / ".config/claudex/providers.json"
    return installed if installed.is_file() else REPOSITORY_CONFIG


def load_config(path: Path) -> dict[str, Any]:
    """Read and minimally validate the shared provider configuration."""
    config = json.loads(path.read_text(encoding="utf-8"))
    if config.get("version") != 1:
        raise ValueError("provider config version must be 1")
    providers = config.get("providers")
    if not isinstance(providers, list) or not providers:
        raise ValueError("provider config must contain providers")
    enabled = [provider for provider in providers if provider.get("enabled", True)]
    if not enabled or any(not valid_provider(provider) for provider in enabled):
        raise ValueError("provider config contains an invalid enabled provider")
    if config.get("mainProvider") not in {provider["id"] for provider in enabled}:
        raise ValueError("mainProvider must name an enabled provider")
    if not valid_choice(config.get("fallback")):
        raise ValueError("provider config contains an invalid fallback")
    return {**config, "providers": enabled}


def valid_provider(provider: Any) -> bool:
    """Check fields used by both quota and model routing."""
    required = ("id", "agent", "defaultModel", "effort", "backend")
    return isinstance(provider, dict) and all(
        isinstance(provider.get(field), str) and provider[field] for field in required
    )


def valid_choice(choice: Any) -> bool:
    """Check the native fallback agent selection."""
    return isinstance(choice, dict) and all(
        isinstance(choice.get(field), str) and choice[field]
        for field in ("agent", "model", "effort")
    )


def configuration_key(config: dict[str, Any]) -> str:
    """Bind cached capacity decisions to the exact routing configuration."""
    compact = json.dumps(config, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(compact).hexdigest()


def usage_percentages(value: Any) -> list[float]:
    """Return every numeric usedPercent value from a provider usage tree."""
    percentages: list[float] = []
    if isinstance(value, dict):
        for key, nested in value.items():
            if key == "usedPercent" and valid_percentage(nested):
                percentages.append(float(nested))
            else:
                percentages.extend(usage_percentages(nested))
    elif isinstance(value, list):
        for nested in value:
            percentages.extend(usage_percentages(nested))
    return percentages


def valid_percentage(value: Any) -> bool:
    """Accept finite, non-negative percentages while rejecting booleans."""
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and math.isfinite(value)
        and value >= 0
    )


def provider_status(report: Any, provider: str) -> dict[str, Any]:
    """Reduce one provider to the non-sensitive fields needed for routing."""
    entries = report if isinstance(report, list) else []
    entry = next(
        (
            item
            for item in entries
            if isinstance(item, dict)
            and str(item.get("provider", "")).casefold() == provider.casefold()
        ),
        None,
    )
    if entry is None:
        return status(False, None, "missing")
    if "available" in entry:
        return explicitly_reported_status(entry)
    percentages = usage_percentages(entry.get("usage"))
    if not percentages:
        return status(False, None, "unknown")
    maximum = max(percentages)
    return status(maximum < 100, maximum, "available" if maximum < 100 else "exhausted")


def explicitly_reported_status(entry: dict[str, Any]) -> dict[str, Any]:
    """Read availability and optional local usage from a non-Codexbar source."""
    available = entry.get("available")
    if not isinstance(available, bool):
        return status(False, None, "unknown")
    reason = entry.get("reason")
    if not isinstance(reason, str) or not reason:
        reason = "available" if available else "usage-unavailable"
    result = status(available, None, reason)
    local = entry.get("localUsage")
    if not isinstance(local, dict):
        return result
    period = local.get("period")
    tokens = local.get("tokens")
    requests = local.get("requests")
    if (
        isinstance(period, str)
        and period
        and numeric_usage_value(tokens)
        and numeric_usage_value(requests)
    ):
        result.update(
            {
                "usage_period": period,
                "usage_tokens": int(tokens),
                "usage_requests": int(requests),
            }
        )
    return result


def numeric_usage_value(value: Any) -> bool:
    """Accept non-negative integer-like usage counters while rejecting booleans."""
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and value >= 0
        and float(value).is_integer()
    )


def status(available: bool, maximum: float | None, reason: str) -> dict[str, Any]:
    """Create the stable, sanitized quota status shape."""
    return {
        "available": available,
        "max_used_percent": maximum,
        "remaining_percent": None if maximum is None else max(0.0, 100 - maximum),
        "reason": reason,
    }


def worker(provider: dict[str, Any]) -> dict[str, Any]:
    """Expose only the routing fields an orchestrator needs."""
    return {
        "provider": provider["id"],
        "agent": provider["agent"],
        "model": provider["defaultModel"],
        "effort": provider["effort"],
        "model_prefixes": provider.get("modelPrefixes", []),
    }


def capacity_priority(quota: dict[str, Any], config_index: int) -> tuple[float, ...]:
    """Prefer unmetered, then greater known headroom, then unknown-limit providers."""
    if quota["reason"] == "unmetered":
        return (0, config_index)
    maximum = quota["max_used_percent"]
    if isinstance(maximum, (int, float)):
        return (1, float(maximum), config_index)
    return (2, config_index)


def routing_summary(
    report: Any, config: dict[str, Any] | None = None
) -> dict[str, Any]:
    """Select configured workers when they have capacity, otherwise fallback."""
    config = config or load_config(config_path(os.environ))
    providers: dict[str, dict[str, Any]] = {}
    candidates: list[tuple[tuple[float, ...], dict[str, Any]]] = []
    for index, provider in enumerate(config["providers"]):
        quota_name = provider.get("usageProvider")
        quota = (
            provider_status(report, quota_name)
            if isinstance(quota_name, str) and quota_name
            else status(True, None, "unmetered")
        )
        providers[provider["id"]] = {**quota, **worker(provider)}
        if quota["available"]:
            candidates.append((capacity_priority(quota, index), worker(provider)))
    selected = [
        item for _, item in sorted(candidates, key=lambda candidate: candidate[0])
    ]
    fallback_active = not selected
    if fallback_active:
        selected = [{"provider": "fallback", **config["fallback"]}]
    return {
        "providers": providers,
        "selected_agents": [item["agent"] for item in selected],
        "selected_workers": selected,
        "preferred_worker": selected[0],
        "fallback_active": fallback_active,
    }


def hook_output(summary: dict[str, Any]) -> dict[str, Any]:
    """Wrap the routing summary in Claude Code's structured hook response."""
    compact = json.dumps(summary, ensure_ascii=False, separators=(",", ":"))
    instructions = (
        " Follow claudex-routing: delegation is the standing default for substantive work unless "
        "the user opts out; do not wait for them to repeat it or merely announce future delegation. "
        "Use selected_workers and pass each "
        "worker's model and effort as claudex_model and claudex_effort for every Agent/Task launch, "
        "prefer preferred_worker for primary work because known quota headroom orders the list, "
        "including nested launches from a worker; never default a nested launch to generic claude "
        "or blindly inherit its parent route. If the user names a "
        "model matching model_prefixes, dynamically select that provider and pass the exact "
        "requested model. This current routing context overrides stale auto-memory about worker "
        "model policy; do not inspect such memory before delegating. Use Claude Code's built-in "
        "parameterless advisor tool according to its standard policy; it is independent of provider "
        "capacity and already receives the complete conversation history. "
        "Start as many worker instances as useful, but for related follow-ups use "
        "SendMessage with the exact compatible recipient specified by the prior Agent/Task result; "
        "decide shutdown only after weighing likely reuse and potential cache value against resource pressure. "
        "Prefer foreground parallel calls when their results are needed now; use background only "
        "when useful work can continue or the task should outlive the turn. TUI N queued is pending "
        "main-session input, including human prompts and background notifications, not worker "
        "capacity, active slots, or SendMessage delivery."
    )
    return {
        "hookSpecificOutput": {
            "hookEventName": "UserPromptSubmit",
            "additionalContext": f"Claudex routing for this turn: {compact}.{instructions}",
        }
    }


def read_cache(
    path: Path, now: float, ttl: int, expected_key: str | None = None
) -> dict[str, Any] | None:
    """Read a fresh, already-sanitized routing summary for this config."""
    if ttl <= 0:
        return None
    try:
        cached = json.loads(path.read_text(encoding="utf-8"))
        if expected_key is not None and cached.get("configuration_key") != expected_key:
            return None
        if now - float(cached["created_at"]) <= ttl:
            return cached["summary"]
    except (FileNotFoundError, KeyError, TypeError, ValueError, json.JSONDecodeError):
        pass
    return None


def write_cache(
    path: Path, summary: dict[str, Any], now: float, key: str | None = None
) -> None:
    """Atomically cache only the sanitized summary, never raw Codexbar output."""
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = json.dumps(
        {"created_at": now, "configuration_key": key, "summary": summary},
        separators=(",", ":"),
    )
    temporary: str | None = None
    try:
        with tempfile.NamedTemporaryFile(
            "w", encoding="utf-8", dir=path.parent, delete=False
        ) as handle:
            temporary = handle.name
            handle.write(payload)
        os.chmod(temporary, 0o600)
        os.replace(temporary, path)
    finally:
        if temporary:
            Path(temporary).unlink(missing_ok=True)


def run_codexbar(program: str) -> Any:
    """Load Codexbar JSON without involving a shell."""
    completed = subprocess.run(
        [program, "usage", "--json"],
        check=True,
        capture_output=True,
        text=True,
        timeout=USAGE_COMMAND_TIMEOUT_SECONDS,
    )
    return json.loads(completed.stdout)


def qwen_usage_entry(payload: Any, provider: str, model: str) -> dict[str, Any]:
    """Convert Qwen's local monthly export to a sanitized provider report."""
    if not isinstance(payload, dict):
        raise TypeError("Qwen usage export must be an object")
    period = payload.get("value")
    models = payload.get("byModel")
    if (
        payload.get("period") != "month"
        or not isinstance(period, str)
        or re.fullmatch(r"\d{4}-\d{2}", period) is None
        or not isinstance(models, list)
    ):
        raise ValueError("Qwen usage export is missing period or model usage")
    model_usage = next(
        (
            item
            for item in models
            if isinstance(item, dict) and item.get("model") == model
        ),
        None,
    )
    tokens = 0 if model_usage is None else model_usage.get("totalTokens")
    requests = 0 if model_usage is None else model_usage.get("requests")
    if not numeric_usage_value(tokens) or not numeric_usage_value(requests):
        raise ValueError("Qwen usage export contains invalid counters")
    return {
        "provider": provider,
        "available": True,
        "reason": "available-local-stats-only",
        "localUsage": {
            "period": period,
            "tokens": int(tokens),
            "requests": int(requests),
        },
    }


def run_qwen_usage(program: str, provider: str, model: str) -> dict[str, Any]:
    """Ask Qwen Code to export its local monthly usage without a model request."""
    with tempfile.TemporaryDirectory(prefix="claudex-qwen-usage-") as directory:
        subprocess.run(
            [
                program,
                "--prompt",
                f"/stats export monthly --format json --output {QWEN_USAGE_FILENAME}",
                "--output-format",
                "text",
                "--safe-mode",
            ],
            check=True,
            capture_output=True,
            text=True,
            timeout=USAGE_COMMAND_TIMEOUT_SECONDS,
            cwd=directory,
        )
        payload = json.loads(
            (Path(directory) / QWEN_USAGE_FILENAME).read_text(encoding="utf-8")
        )
    return qwen_usage_entry(payload, provider, model)


def unavailable_usage_entry(provider: str) -> dict[str, Any]:
    """Represent a provider-specific usage command failure."""
    return {
        "provider": provider,
        "available": False,
        "reason": "usage-unavailable",
    }


def collect_usage(
    config: dict[str, Any], codexbar_program: str, qwen_program: str
) -> list[dict[str, Any]]:
    """Collect Codexbar and Qwen usage independently so one failure stays isolated."""
    providers = config["providers"]
    qwen_providers = [
        provider
        for provider in providers
        if str(provider.get("usageProvider", "")).casefold() == QWEN_USAGE_PROVIDER
    ]
    qwen_names = {provider["usageProvider"].casefold() for provider in qwen_providers}
    codexbar_names = {
        provider["usageProvider"]
        for provider in providers
        if isinstance(provider.get("usageProvider"), str)
        and provider["usageProvider"]
        and provider not in qwen_providers
    }
    try:
        raw_report = run_codexbar(codexbar_program)
        report = (
            [
                entry
                for entry in raw_report
                if not isinstance(entry, dict)
                or str(entry.get("provider", "")).casefold() not in qwen_names
            ]
            if isinstance(raw_report, list)
            else []
        )
    except (OSError, ValueError, subprocess.SubprocessError, json.JSONDecodeError):
        report = [unavailable_usage_entry(name) for name in sorted(codexbar_names)]
    for provider in qwen_providers:
        try:
            report.append(
                run_qwen_usage(
                    qwen_program,
                    provider["usageProvider"],
                    provider["defaultModel"],
                )
            )
        except (
            OSError,
            TypeError,
            ValueError,
            subprocess.SubprocessError,
            json.JSONDecodeError,
        ):
            report.append(unavailable_usage_entry(provider["usageProvider"]))
    return report


def cache_seconds(environment: dict[str, str]) -> int:
    """Parse the optional cache TTL, falling back safely on invalid values."""
    try:
        return max(
            0,
            int(environment.get("CLAUDEX_USAGE_CACHE_SECONDS", DEFAULT_CACHE_SECONDS)),
        )
    except ValueError:
        return DEFAULT_CACHE_SECONDS


def fallback_summary(
    reason: str, config: dict[str, Any] | None = None
) -> dict[str, Any]:
    """Prefer the configured native fallback when usage cannot be established."""
    config = config or load_config(config_path(os.environ))
    providers = {
        provider["id"]: {**status(False, None, reason), **worker(provider)}
        for provider in config["providers"]
    }
    fallback = {"provider": "fallback", **config["fallback"]}
    return {
        "providers": providers,
        "selected_agents": [fallback["agent"]],
        "selected_workers": [fallback],
        "preferred_worker": fallback,
        "fallback_active": True,
    }


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, help="provider routing JSON")
    parser.add_argument("--input", type=Path, help="read a Codexbar JSON fixture")
    parser.add_argument("--no-cache", action="store_true")
    parser.add_argument("--codexbar-program", default="codexbar")
    parser.add_argument("--qwen-program", default="qwen")
    return parser.parse_args()


def main() -> int:
    arguments = parse_arguments()
    now = time.time()
    try:
        config = load_config(config_path(os.environ, arguments.config))
    except (OSError, ValueError, json.JSONDecodeError) as error:
        raise SystemExit(f"claudex routing configuration error: {error}") from error
    key = configuration_key(config)
    cache_path = Path.home() / ".cache/claudex/usage-routing.json"
    ttl = 0 if arguments.no_cache or arguments.input else cache_seconds(os.environ)
    summary = read_cache(cache_path, now, ttl, key)
    if summary is None:
        try:
            report = (
                json.loads(arguments.input.read_text(encoding="utf-8"))
                if arguments.input
                else collect_usage(
                    config, arguments.codexbar_program, arguments.qwen_program
                )
            )
            summary = routing_summary(report, config)
            if ttl > 0:
                write_cache(cache_path, summary, now, key)
        except (OSError, ValueError, subprocess.SubprocessError, json.JSONDecodeError):
            summary = fallback_summary("usage-unavailable", config)
    print(json.dumps(hook_output(summary), ensure_ascii=False, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
