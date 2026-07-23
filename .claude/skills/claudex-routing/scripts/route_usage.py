#!/usr/bin/env python3
"""Emit sanitized Claude hook context from `codexbar usage --json`."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any

DEFAULT_CACHE_SECONDS = 300
PROVIDER_AGENTS = {"codex": "claudex-gpt", "grok": "claudex-grok"}


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
        return {"available": False, "max_used_percent": None, "reason": "missing"}

    percentages = usage_percentages(entry.get("usage"))
    if not percentages:
        return {"available": False, "max_used_percent": None, "reason": "unknown"}

    maximum = max(percentages)
    return {
        "available": maximum < 100,
        "max_used_percent": maximum,
        "reason": "available" if maximum < 100 else "exhausted",
    }


def routing_summary(report: Any) -> dict[str, Any]:
    """Select external workers when they have capacity, otherwise Sonnet."""
    providers = {
        provider: provider_status(report, provider) for provider in PROVIDER_AGENTS
    }
    selected = [
        PROVIDER_AGENTS[provider]
        for provider in PROVIDER_AGENTS
        if providers[provider]["available"]
    ]
    return {
        "providers": providers,
        "selected_agents": selected or ["claudex-sonnet"],
        "fallback_active": not selected,
    }


def hook_output(summary: dict[str, Any]) -> dict[str, Any]:
    """Wrap the routing summary in Claude Code's structured hook response."""
    compact = json.dumps(summary, ensure_ascii=False, separators=(",", ":"))
    return {
        "hookSpecificOutput": {
            "hookEventName": "UserPromptSubmit",
            "additionalContext": (
                "Claudex capacity routing for this turn: "
                f"{compact}. Follow the claudex-routing skill: delegate primary work only to "
                "selected_agents, use both GPT and Grok when useful if both are selected, and "
                "use claudex-sonnet as the fallback only when it is selected."
            ),
        }
    }


def read_cache(path: Path, now: float, ttl: int) -> dict[str, Any] | None:
    """Read a fresh, already-sanitized routing summary."""
    if ttl <= 0:
        return None
    try:
        cached = json.loads(path.read_text(encoding="utf-8"))
        if now - float(cached["created_at"]) <= ttl:
            return cached["summary"]
    except (FileNotFoundError, KeyError, TypeError, ValueError, json.JSONDecodeError):
        pass
    return None


def write_cache(path: Path, summary: dict[str, Any], now: float) -> None:
    """Atomically cache only the sanitized summary, never raw Codexbar output."""
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = json.dumps({"created_at": now, "summary": summary}, separators=(",", ":"))
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
        return max(0, int(environment.get("CLAUDEX_USAGE_CACHE_SECONDS", DEFAULT_CACHE_SECONDS)))
    except ValueError:
        return DEFAULT_CACHE_SECONDS


def fallback_summary(reason: str) -> dict[str, Any]:
    """Prefer the Claude subscription when provider usage cannot be established."""
    return {
        "providers": {
            provider: {
                "available": False,
                "max_used_percent": None,
                "reason": reason,
            }
            for provider in PROVIDER_AGENTS
        },
        "selected_agents": ["claudex-sonnet"],
        "fallback_active": True,
    }


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, help="read a Codexbar JSON fixture")
    parser.add_argument("--no-cache", action="store_true")
    parser.add_argument("--codexbar-program", default="codexbar")
    return parser.parse_args()


def main() -> int:
    arguments = parse_arguments()
    now = time.time()
    cache_path = Path.home() / ".cache/claudex/usage-routing.json"
    ttl = 0 if arguments.no_cache or arguments.input else cache_seconds(os.environ)
    summary = read_cache(cache_path, now, ttl)
    if summary is None:
        try:
            report = (
                json.loads(arguments.input.read_text(encoding="utf-8"))
                if arguments.input
                else run_codexbar(arguments.codexbar_program)
            )
            summary = routing_summary(report)
            if ttl > 0:
                write_cache(cache_path, summary, now)
        except (OSError, ValueError, subprocess.SubprocessError, json.JSONDecodeError):
            summary = fallback_summary("usage-unavailable")
    print(json.dumps(hook_output(summary), ensure_ascii=False, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
