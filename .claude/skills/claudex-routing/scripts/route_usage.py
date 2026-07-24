#!/usr/bin/env python3
"""Emit sanitized, config-driven Claude routing context from Codexbar usage."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any

DEFAULT_CACHE_SECONDS = 300
REPOSITORY_CONFIG = Path(__file__).parents[4] / ".config/claudex/providers.json"


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
    for name in ("fallback", "advisor"):
        if not valid_choice(config.get(name)):
            raise ValueError(f"provider config contains an invalid {name}")
    return {**config, "providers": enabled}


def valid_provider(provider: Any) -> bool:
    """Check fields used by both quota and model routing."""
    required = ("id", "agent", "defaultModel", "effort", "backend")
    return isinstance(provider, dict) and all(
        isinstance(provider.get(field), str) and provider[field] for field in required
    )


def valid_choice(choice: Any) -> bool:
    """Check a native fallback or advisor agent selection."""
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
            if key == "usedPercent" and isinstance(nested, (int, float)):
                percentages.append(float(nested))
            else:
                percentages.extend(usage_percentages(nested))
    elif isinstance(value, list):
        for nested in value:
            percentages.extend(usage_percentages(nested))
    return percentages


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
    percentages = usage_percentages(entry.get("usage"))
    if not percentages:
        return status(False, None, "unknown")
    maximum = max(percentages)
    return status(maximum < 100, maximum, "available" if maximum < 100 else "exhausted")


def status(available: bool, maximum: float | None, reason: str) -> dict[str, Any]:
    """Create the stable, sanitized quota status shape."""
    return {
        "available": available,
        "max_used_percent": maximum,
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


def routing_summary(
    report: Any, config: dict[str, Any] | None = None
) -> dict[str, Any]:
    """Select configured workers when they have capacity, otherwise fallback."""
    config = config or load_config(config_path(os.environ))
    providers: dict[str, dict[str, Any]] = {}
    selected: list[dict[str, Any]] = []
    for provider in config["providers"]:
        quota_name = provider.get("usageProvider")
        quota = (
            provider_status(report, quota_name)
            if isinstance(quota_name, str) and quota_name
            else status(True, None, "unmetered")
        )
        providers[provider["id"]] = {**quota, **worker(provider)}
        if quota["available"]:
            selected.append(worker(provider))
    fallback_active = not selected
    if fallback_active:
        selected = [{"provider": "fallback", **config["fallback"]}]
    return {
        "providers": providers,
        "selected_agents": [item["agent"] for item in selected],
        "selected_workers": selected,
        "fallback_active": fallback_active,
        "advisor": config["advisor"],
    }


def hook_output(summary: dict[str, Any]) -> dict[str, Any]:
    """Wrap the routing summary in Claude Code's structured hook response."""
    compact = json.dumps(summary, ensure_ascii=False, separators=(",", ":"))
    instructions = (
        " Follow claudex-routing: use selected_workers for primary delegation and pass each "
        "worker's model and effort as claudex_model and claudex_effort for every Agent/Task launch, "
        "including nested launches from a worker; never default a nested launch to generic claude "
        "or blindly inherit its parent route. If the user names a "
        "model matching model_prefixes, dynamically select that provider and pass the exact "
        "requested model. This current routing context overrides stale auto-memory about worker "
        "or advisor model policy; do not inspect such memory before delegating. The advisor is "
        "independent of capacity: invoke it alongside workers "
        "when explicitly requested or when a complex, ambiguous, or high-risk decision benefits "
        "from strategic review. Start as many instances as useful, but for related follow-ups use "
        "SendMessage with the exact compatible recipient specified by the prior Agent/Task result; "
        "decide shutdown only after weighing likely reuse and potential cache value against resource pressure."
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
        timeout=45,
    )
    return json.loads(completed.stdout)


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
        "fallback_active": True,
        "advisor": config["advisor"],
    }


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, help="provider routing JSON")
    parser.add_argument("--input", type=Path, help="read a Codexbar JSON fixture")
    parser.add_argument("--no-cache", action="store_true")
    parser.add_argument("--codexbar-program", default="codexbar")
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
                else run_codexbar(arguments.codexbar_program)
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
