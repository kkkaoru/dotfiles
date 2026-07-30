#!/usr/bin/env python3
"""Emit sanitized routing context from Codexbar and Qwen Cloud quota."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import shlex
import subprocess
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Mapping
from urllib.parse import parse_qs, urlparse

import opencode_go_budget

DEFAULT_CACHE_SECONDS = 300
# Bump when worker-selection semantics change so a cached context cannot retain
# the old main-model exclusion rule for up to the normal routing-cache TTL.
ROUTING_CACHE_VERSION = 5
QWEN_QUOTA_CACHE_SECONDS = 60 * 60
QWEN_REQUEST_TIMEOUT_SECONDS = 5
QWEN_SUBPROCESS_GRACE_SECONDS = 2
REPOSITORY_ROOT = Path(__file__).parents[4]
REPOSITORY_CONFIG = REPOSITORY_ROOT / ".config/claudex/providers.json"
REPOSITORY_DISABLED_MODELS_CONFIG = (
    REPOSITORY_ROOT / ".config/claudex/disabled-subagent-models.json"
)
DEFAULT_QWEN_CURL = REPOSITORY_ROOT / "tmp/curl.txt"
QWEN_USAGE_PROVIDER = "qwen"
OLLAMA_USAGE_PROVIDER = "ollama"
OLLAMA_BASE_URL_ENV = "CLAUDEX_OLLAMA_BASE_URL"
DEFAULT_OLLAMA_BASE_URL = "http://127.0.0.1:11434"
DAEMON_HEALTH_URL_ENV = "CLAUDEX_DAEMON_HEALTH_URL"
ANTHROPIC_BASE_URL_ENV = "ANTHROPIC_BASE_URL"
DEFAULT_DAEMON_HEALTH_URL = "http://127.0.0.1:8318/health"
DAEMON_HEALTH_TIMEOUT_SECONDS = 2
QWEN_CONSOLE_HOST = "cs-data.qwencloud.com"
QWEN_CONSOLE_PATH = "/data/api.json"
QWEN_CONSOLE_PRODUCT = "sfm_bailian"
QWEN_CONSOLE_ACTION = "IntlBroadScopeAspnGateway"
QWEN_QUOTA_API = "zeldaHttp.apikeyMgr./tokenplan/personal/api/v2/usage"
USAGE_COMMAND_TIMEOUT_SECONDS = 45
DISABLED_SUBAGENT_MODELS_ENV = "CLAUDEX_DISABLED_SUBAGENT_MODELS"
DISABLED_SUBAGENT_MODELS_CONFIG_ENV = "CLAUDEX_DISABLED_SUBAGENT_MODELS_CONFIG"
RESOLVED_DISABLED_SUBAGENT_MODELS_ENV = "CLAUDEX_RESOLVED_DISABLED_SUBAGENT_MODELS"
CUSTOM_ADVISOR_ENV = "CLAUDEX_CUSTOM_ADVISOR"
CUSTOM_ADVISOR_DISABLED_VALUES = frozenset({"0", "false", "off"})
OUTER_MODEL_ENV = "CLAUDEX_OUTER_MODEL"
ALLOW_SONNET_SUBAGENT_ENV = "CLAUDEX_ALLOW_SONNET_SUBAGENT"
# Claude Code settings use the short `sonnet[1m]` spelling while the
# configured fallback worker uses the canonical `claude-sonnet-5` ID.  Keep
# the equivalence local to routing; explicit Agent/Task model requests still
# pass through the normal denylist and provider validation paths.
SONNET_MODEL_ALIASES = frozenset(
    {
        "sonnet",
        "sonnet[1m]",
        "claude-sonnet-5",
        "claude-sonnet-5[1m]",
    }
)
# These values describe the orchestration contract injected into Claude Code.
# The routing hook cannot start Agent/Task calls itself; the main session uses
# this metadata to choose and rebalance ordinary workers.
MIN_SUBAGENT_FANOUT = 3
MIN_ACTIVE_SUBAGENTS = 2
MIN_SUBAGENT_MODEL_KINDS = 2
ORCHESTRATION_REBALANCE_INTERVAL_SECONDS = 10 * 60
DEFAULT_SUBAGENT_STATUS_POLL_SECONDS = 15
SUBAGENT_MIN_PARALLEL_ENV = "CLAUDEX_SUBAGENT_MIN_PARALLEL"
SUBAGENT_ACTIVE_FLOOR_ENV = "CLAUDEX_SUBAGENT_ACTIVE_FLOOR"
SUBAGENT_REEVALUATE_ON_COMPLETION_ENV = "CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION"
SUBAGENT_REASSESS_INTERVAL_ENV = "CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS"
SUBAGENT_MIN_MODEL_FAMILIES_ENV = "CLAUDEX_SUBAGENT_MIN_MODEL_FAMILIES"
SUBAGENT_REUSE_ENV = "CLAUDEX_SUBAGENT_REUSE"
SUBAGENT_CLEANUP_ON_EXIT_ENV = "CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT"
SUBAGENT_FIRST_ENV = "CLAUDEX_SUBAGENT_FIRST"
SUBAGENT_STATUS_POLL_ENV = "CLAUDEX_SUBAGENT_STATUS_POLL_SECONDS"
DEFAULT_ADVISOR = {
    "agent": "custom-advisor",
    "model": "claude-fable-5",
    "effort": "xhigh",
}
CUSTOM_ADVISOR_CONSULT_WHEN = (
    "complex_or_ambiguous_decision",
    "external_research_or_multiple_sources",
    "high_risk_implementation_or_config_change",
    "long_running_phase_over_ten_minutes",
    "worker_failure_timeout_or_stall",
    "conflicting_worker_results",
)


def config_path(environment: Mapping[str, str], requested: Path | None = None) -> Path:
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
    main_providers = config.get("mainProviders")
    if (
        not isinstance(main_providers, list)
        or not main_providers
        or any(not isinstance(provider, str) for provider in main_providers)
        or len(set(main_providers)) != len(main_providers)
        or any(provider not in {item["id"] for item in enabled} for provider in main_providers)
    ):
        raise ValueError("mainProviders must name distinct enabled providers")
    if not valid_choice(config.get("fallback")):
        raise ValueError("provider config contains an invalid fallback")
    advisor = config.get("advisor", DEFAULT_ADVISOR)
    if not valid_choice(advisor):
        raise ValueError("provider config contains an invalid advisor")
    return {**config, "providers": enabled, "advisor": dict(advisor)}


def disabled_models_config_path(
    environment: Mapping[str, str], requested: Path | None = None
) -> Path:
    """Resolve the dedicated model-policy config independently of providers."""
    if requested:
        return requested
    if DISABLED_SUBAGENT_MODELS_CONFIG_ENV in environment:
        configured = environment[DISABLED_SUBAGENT_MODELS_CONFIG_ENV]
        if not configured:
            raise ValueError(f"{DISABLED_SUBAGENT_MODELS_CONFIG_ENV} must not be empty")
        return Path(configured).expanduser()
    installed = Path.home() / ".config/claudex/disabled-subagent-models.json"
    return installed if installed.is_file() else REPOSITORY_DISABLED_MODELS_CONFIG


def valid_provider(provider: Any) -> bool:
    """Check fields used by both quota and model routing."""
    required = ("id", "agent", "defaultModel", "effort", "backend")
    if not isinstance(provider, dict) or not all(
        isinstance(provider.get(field), str) and provider[field] for field in required
    ):
        return False
    if "subagentModel" in provider and not valid_model_id(provider["subagentModel"]):
        return False
    maximum = provider.get("maxConcurrency")
    valid_maximum = maximum is None or (
        isinstance(maximum, int) and not isinstance(maximum, bool) and maximum > 0
    )
    if not valid_maximum:
        return False
    budget = provider.get("requestBudget")
    if budget is None:
        return True
    return (
        provider.get("defaultModel") == opencode_go_budget.DEFAULT_MODEL
        and str(provider.get("usageProvider", "")).casefold()
        == opencode_go_budget.DEFAULT_USAGE_PROVIDER
        and opencode_go_budget.valid_request_budget(budget)
    )


def valid_choice(choice: Any) -> bool:
    """Check the native fallback or advisor agent selection."""
    return isinstance(choice, dict) and all(
        isinstance(choice.get(field), str) and choice[field]
        for field in ("agent", "model", "effort")
    )


def valid_model_id(model: Any) -> bool:
    """Accept a non-empty, visible-ASCII exact model identifier."""
    return (
        isinstance(model, str)
        and bool(model)
        and model.isascii()
        and all("!" <= char <= "~" for char in model)
    )


def model_family(model: str) -> str:
    for separator in ("/", "-", "_", "."):
        model = model.split(separator, 1)[0]
    return model


def load_disabled_models_config(path: Path) -> frozenset[str]:
    """Read and validate the dedicated exact-model denylist."""
    policy = json.loads(path.read_text(encoding="utf-8"))
    if (
        not isinstance(policy, dict)
        or set(policy) != {"version", "disabledModels"}
        or policy["version"] != 1
    ):
        raise ValueError("disabled SubAgent model config must use version 1 schema")
    configured = policy["disabledModels"]
    if not isinstance(configured, list) or any(
        not valid_model_id(model) for model in configured
    ):
        raise ValueError("disabledModels must contain valid exact model IDs")
    if len(set(configured)) != len(configured):
        raise ValueError("disabledModels must not contain duplicates")
    return frozenset(configured)


def disabled_subagent_models(
    configured: frozenset[str], environment: Mapping[str, str]
) -> frozenset[str]:
    """Merge configured models with the terminal-local exact model denylist."""
    return configured | environment_models(environment, DISABLED_SUBAGENT_MODELS_ENV)


def custom_advisor_enabled(environment: Mapping[str, str] | None = None) -> bool:
    """Return whether the independent custom-advisor channel is enabled."""
    values = os.environ if environment is None else environment
    return values.get(CUSTOM_ADVISOR_ENV, "").strip().casefold() not in CUSTOM_ADVISOR_DISABLED_VALUES


def environment_models(environment: Mapping[str, str], name: str) -> frozenset[str]:
    """Parse one comma-separated exact-model environment value."""
    models = {
        item.strip()
        for item in environment.get(name, "").split(",")
        if item.strip()
    }
    if any(not valid_model_id(model) for model in models):
        raise ValueError(f"{name} contains an invalid model ID")
    return frozenset(models)


def configuration_key(
    config: dict[str, Any], disabled_models: frozenset[str] = frozenset()
) -> str:
    """Bind cached capacity decisions to the config and terminal model policy."""
    compact = json.dumps(
        {
            "cacheVersion": ROUTING_CACHE_VERSION,
            "config": config,
            "disabledSubagentModels": sorted(disabled_models),
        },
        sort_keys=True,
        separators=(",", ":"),
    ).encode()
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


def provider_quota_status(
    report: Any, provider: dict[str, Any]
) -> dict[str, Any]:
    """Evaluate a provider's published request budget before generic usage."""
    usage_provider = provider.get("usageProvider")
    if not isinstance(usage_provider, str) or not usage_provider:
        return status(True, None, "unmetered")
    budget = provider.get("requestBudget")
    if budget is not None:
        evaluated = opencode_go_budget.evaluate(report, usage_provider, budget)
        if evaluated is not None:
            return evaluated
    return provider_status(report, usage_provider)


def explicitly_reported_status(entry: dict[str, Any]) -> dict[str, Any]:
    """Read availability and optional quota usage from a non-Codexbar source."""
    available = entry.get("available")
    if not isinstance(available, bool):
        return status(False, None, "unknown")
    reason = entry.get("reason")
    if not isinstance(reason, str) or not reason:
        reason = "available" if available else "usage-unavailable"
    maximum = entry.get("maxUsedPercent")
    if maximum is None:
        return status(available, None, reason)
    if not valid_percentage(maximum) or float(maximum) > 100:
        return status(False, None, "unknown")
    return status(available, float(maximum), reason)


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
    result = {
        "provider": provider["id"],
        "agent": provider["agent"],
        "model": provider.get("subagentModel", provider["defaultModel"]),
        "effort": provider["effort"],
        "model_prefixes": provider.get("modelPrefixes", []),
    }
    if "maxConcurrency" in provider:
        result["max_concurrency"] = provider["maxConcurrency"]
    return result


def _positive_or_default(
    environment: Mapping[str, str], name: str, default: int, minimum: int
) -> int:
    """Parse one positive orchestration integer and reject unsafe values."""
    raw = environment.get(name)
    if raw is None or not raw.strip():
        return default
    try:
        value = int(raw)
    except (TypeError, ValueError) as error:
        raise ValueError(f"{name} must be an integer >= {minimum}") from error
    if value < minimum:
        raise ValueError(f"{name} must be an integer >= {minimum}")
    return value


def _boolean_or_default(
    environment: Mapping[str, str], name: str, default: bool
) -> bool:
    """Parse a strict boolean orchestration switch."""
    raw = environment.get(name)
    if raw is None or not raw.strip():
        return default
    normalized = raw.strip().casefold()
    if normalized in {"1", "true", "yes", "on"}:
        return True
    if normalized in {"0", "false", "no", "off"}:
        return False
    raise ValueError(f"{name} must be one of 0, 1, true, or false")


def orchestration_settings(environment: Mapping[str, str] | None = None) -> dict[str, Any]:
    """Resolve validated worker lifecycle settings from terminal-local env."""
    values = os.environ if environment is None else environment
    return {
        "minimum_subagents_per_phase": _positive_or_default(
            values, SUBAGENT_MIN_PARALLEL_ENV, MIN_SUBAGENT_FANOUT, MIN_SUBAGENT_FANOUT
        ),
        "minimum_active_subagents": _positive_or_default(
            values, SUBAGENT_ACTIVE_FLOOR_ENV, MIN_ACTIVE_SUBAGENTS, MIN_ACTIVE_SUBAGENTS
        ),
        "reevaluate_on_completion": _boolean_or_default(
            values, SUBAGENT_REEVALUATE_ON_COMPLETION_ENV, True
        ),
        "monitor_interval_seconds": _positive_or_default(
            values,
            SUBAGENT_REASSESS_INTERVAL_ENV,
            ORCHESTRATION_REBALANCE_INTERVAL_SECONDS,
            1,
        ),
        "minimum_model_kinds": _positive_or_default(
            values,
            SUBAGENT_MIN_MODEL_FAMILIES_ENV,
            MIN_SUBAGENT_MODEL_KINDS,
            MIN_SUBAGENT_MODEL_KINDS,
        ),
        "reuse_compatible_workers": _boolean_or_default(values, SUBAGENT_REUSE_ENV, True),
        "cleanup_on_exit": _boolean_or_default(
            values, SUBAGENT_CLEANUP_ON_EXIT_ENV, True
        ),
        "subagent_first": _boolean_or_default(values, SUBAGENT_FIRST_ENV, True),
        "status_poll_interval_seconds": _positive_or_default(
            values, SUBAGENT_STATUS_POLL_ENV, DEFAULT_SUBAGENT_STATUS_POLL_SECONDS, 1
        ),
    }


def orchestration_contract(
    summary: Mapping[str, Any], environment: Mapping[str, str] | None = None
) -> dict[str, Any]:
    """Describe the main-session worker contract without launching workers.

    The UserPromptSubmit hook is a context producer, not an Agent/Task
    executor.  Claude Code (or another compatible harness) uses this
    sanitized state to choose the number of independent workstreams and to
    rebalance them as results arrive.
    """
    selected = summary.get("selected_workers")
    workers = selected if isinstance(selected, list) else []
    models = {
        model_family(worker_item["model"])
        for worker_item in workers
        if isinstance(worker_item, dict)
        and isinstance(worker_item.get("model"), str)
        and worker_item["model"]
    }
    available = len(workers)
    settings = orchestration_settings(environment)
    return {
        "dynamic_fanout": True,
        **settings,
        "max_available_workers": available,
        "available_model_kinds": len(models),
        "model_diversity_satisfied": len(models) >= settings["minimum_model_kinds"],
        "completion_rebalance_required": settings["reevaluate_on_completion"],
        "custom_advisor_exempt": True,
        "custom_advisor_consult_when": list(CUSTOM_ADVISOR_CONSULT_WHEN),
        "capacity_shortfall": available < settings["minimum_subagents_per_phase"],
        "hook_launches_agents": False,
        "background_status_required": True,
        "automatic_selection_excluded_models": sorted(
            summary.get("automatic_selection_excluded_models", [])
        ),
        "sonnet_subagent_suppressed": bool(
            summary.get("sonnet_subagent_suppressed", False)
        ),
    }


def is_sonnet_model(model: object) -> bool:
    """Return whether a model spelling identifies the Sonnet 5 family."""
    return isinstance(model, str) and model.strip().casefold() in SONNET_MODEL_ALIASES


def daemon_health_url(environment: Mapping[str, str]) -> str:
    """Accept only the shared loopback daemon health endpoint."""
    if configured := environment.get(DAEMON_HEALTH_URL_ENV):
        return validate_daemon_health_url(configured)
    if base_url := environment.get(ANTHROPIC_BASE_URL_ENV):
        parsed = urlparse(base_url)
        if parsed.hostname in {"127.0.0.1", "::1", "localhost"}:
            origin = parsed._replace(path="/health", params="", query="", fragment="")
            return validate_daemon_health_url(origin.geturl())
    return validate_daemon_health_url(DEFAULT_DAEMON_HEALTH_URL)


def validate_daemon_health_url(value: str) -> str:
    """Reject credentials, remote hosts, and non-health paths."""
    parsed = urlparse(value)
    if (
        parsed.scheme != "http"
        or parsed.hostname not in {"127.0.0.1", "::1", "localhost"}
        or parsed.path != "/health"
        or parsed.params
        or parsed.query
        or parsed.fragment
        or parsed.username is not None
        or parsed.password is not None
    ):
        raise ValueError("daemon health URL must be a loopback HTTP /health endpoint")
    return value


def run_daemon_health(
    curl_program: str, environment: Mapping[str, str]
) -> dict[str, Any] | None:
    """Read public daemon capacity without retaining unrelated health fields."""
    try:
        completed = subprocess.run(
            [
                curl_program,
                "--fail",
                "--silent",
                "--show-error",
                "--max-time",
                str(DAEMON_HEALTH_TIMEOUT_SECONDS),
                daemon_health_url(environment),
            ],
            check=True,
            capture_output=True,
            text=True,
            timeout=DAEMON_HEALTH_TIMEOUT_SECONDS + QWEN_SUBPROCESS_GRACE_SECONDS,
        )
        payload = json.loads(completed.stdout)
        if not isinstance(payload, dict) or payload.get("status") != "ok":
            return None
        return sanitize_model_concurrency(payload.get("model_concurrency"))
    except (
        OSError,
        TypeError,
        ValueError,
        subprocess.SubprocessError,
        json.JSONDecodeError,
    ):
        return None


def sanitize_model_concurrency(value: Any) -> dict[str, dict[str, Any]] | None:
    """Validate the exact-model counters exported by the adapter."""
    if not isinstance(value, dict):
        return None
    sanitized: dict[str, dict[str, Any]] = {}
    for model, fields in value.items():
        if not valid_model_id(model) or not isinstance(fields, dict):
            return None
        active = fields.get("active")
        queued = fields.get("queued")
        limit = fields.get("limit")
        available = fields.get("available")
        if (
            not isinstance(active, int)
            or isinstance(active, bool)
            or active < 0
            or not isinstance(queued, int)
            or isinstance(queued, bool)
            or queued < 0
            or not isinstance(limit, int)
            or isinstance(limit, bool)
            or limit <= 0
            or not isinstance(available, bool)
        ):
            return None
        sanitized[model] = {
            "active": active,
            "queued": queued,
            "limit": limit,
            "available": available and active + queued < limit,
        }
    return sanitized


def provider_for_model(
    config: dict[str, Any], model: str
) -> dict[str, Any] | None:
    """Resolve exact models first, then the most-specific configured prefix."""
    exact = [
        provider
        for provider in config["providers"]
        if model
        in {
            provider["defaultModel"],
            provider.get("subagentModel", provider["defaultModel"]),
        }
    ]
    if exact:
        return exact[0]
    matches = [
        (len(prefix), -index, provider)
        for index, provider in enumerate(config["providers"])
        for prefix in provider.get("modelPrefixes", [])
        if model.startswith(prefix)
    ]
    return max(matches, default=(0, 0, None), key=lambda item: item[:2])[2]


def model_concurrency_status(
    provider: dict[str, Any],
    model: str,
    health: dict[str, dict[str, Any]] | None,
) -> dict[str, Any]:
    """Resolve one model limit, assuming launchable state when health is absent."""
    configured_limit = provider.get("maxConcurrency")
    if configured_limit is None:
        return concurrency_status(None, None, None, True, "not-limited", True)
    if health is None:
        return concurrency_status(
            None, None, configured_limit, True, "daemon-health-unavailable", False
        )
    fields = health.get(model)
    if fields is None:
        return concurrency_status(0, 0, configured_limit, True, "idle", True)
    if fields["limit"] != configured_limit:
        available = fields["active"] + fields["queued"] < configured_limit
        return concurrency_status(
            fields["active"],
            fields["queued"],
            configured_limit,
            available,
            "configured-limit-mismatch",
            False,
        )
    return concurrency_status(
        fields["active"],
        fields["queued"],
        configured_limit,
        fields["available"],
        "available" if fields["available"] else "limit-reached",
        True,
    )


def concurrency_status(
    active: int | None,
    queued: int | None,
    limit: int | None,
    available: bool,
    reason: str,
    known: bool,
) -> dict[str, Any]:
    """Build the sanitized concurrency status attached to routing output."""
    return {
        "active": active,
        "queued": queued,
        "limit": limit,
        "available": available,
        "remaining": (
            None
            if active is None or queued is None or limit is None
            else max(0, limit - active - queued)
        ),
        "reason": reason,
        "known": known,
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
    report: Any,
    config: dict[str, Any] | None = None,
    disabled_models: frozenset[str] = frozenset(),
) -> dict[str, Any]:
    """Select configured workers when they have capacity, otherwise fallback."""
    config = config or load_config(config_path(os.environ))
    providers: dict[str, dict[str, Any]] = {}
    main_workers: dict[str, dict[str, Any]] = {}
    candidates: list[tuple[tuple[float, ...], dict[str, Any]]] = []
    for index, provider in enumerate(config["providers"]):
        quota_name = provider.get("usageProvider")
        quota = provider_quota_status(report, provider)
        disabled = worker(provider)["model"] in disabled_models
        effective = status(False, None, "disabled-by-policy") if disabled else quota
        providers[provider["id"]] = {**effective, **worker(provider), "disabled": disabled}
        main_workers[provider["id"]] = {
            **quota,
            **worker(provider),
            "model": provider["defaultModel"],
        }
        if quota["available"] and not disabled:
            candidates.append((capacity_priority(quota, index), worker(provider)))
    selected = [
        item for _, item in sorted(candidates, key=lambda candidate: candidate[0])
    ]
    fallback_active = not selected and config["fallback"]["model"] not in disabled_models
    if fallback_active:
        selected = [{"provider": "fallback", **config["fallback"]}]
    preferred_main_worker = next(
        (
            main_workers[provider]
            for provider in config["mainProviders"]
            if provider in main_workers and main_workers[provider]["available"]
        ),
        None,
    )
    summary = {
        "providers": providers,
        "main_workers": main_workers,
        "selected_agents": [item["agent"] for item in selected],
        "selected_workers": selected,
        "preferred_worker": selected[0] if selected else None,
        "preferred_main_worker": preferred_main_worker,
        "fallback_active": fallback_active,
        "disabled_subagent_models": sorted(disabled_models),
        "advisor": dict(config.get("advisor", DEFAULT_ADVISOR)),
    }
    summary["orchestration"] = orchestration_contract(summary)
    return summary


def combined_capacity_priority(
    quota: dict[str, Any], concurrency: dict[str, Any], config_index: int
) -> tuple[float, ...]:
    """Rank by the tightest known quota or model-concurrency constraint."""
    if quota["reason"] == "unmetered":
        quota_unknown = 0.0
        quota_used = 0.0
    elif isinstance(quota["max_used_percent"], (int, float)):
        quota_unknown = 0.0
        quota_used = float(quota["max_used_percent"])
    else:
        quota_unknown = 1.0
        quota_used = 0.0
    parallel_used = 0.0
    if (
        concurrency["active"] is not None
        and concurrency["queued"] is not None
        and concurrency["limit"] is not None
    ):
        parallel_used = (
            100
            * (concurrency["active"] + concurrency["queued"])
            / concurrency["limit"]
        )
    health_unknown = 1.0 if concurrency["reason"] == "daemon-health-unavailable" else 0.0
    return (
        quota_unknown,
        max(quota_used, parallel_used),
        health_unknown,
        config_index,
    )


def apply_model_concurrency(
    summary: dict[str, Any],
    config: dict[str, Any],
    health: dict[str, dict[str, Any]] | None,
    disabled_models: frozenset[str] = frozenset(),
) -> dict[str, Any]:
    """Refresh volatile daemon slots without invalidating cached provider usage."""
    combined = json.loads(json.dumps(summary))
    candidates: list[tuple[tuple[float, ...], dict[str, Any]]] = []
    model_capacity: dict[str, dict[str, Any]] = {}
    main_workers = combined.get("main_workers", {})
    for index, provider in enumerate(config["providers"]):
        provider_id = provider["id"]
        fields = combined["providers"][provider_id]
        current_worker = worker(provider)
        model = current_worker["model"]
        concurrency = model_concurrency_status(provider, model, health)
        if "maxConcurrency" in provider:
            model_capacity[model] = concurrency
            fields.update(
                {
                    "concurrency_active": concurrency["active"],
                    "concurrency_queued": concurrency["queued"],
                    "concurrency_limit": concurrency["limit"],
                    "concurrency_remaining": concurrency["remaining"],
                    "concurrency_available": concurrency["available"],
                    "concurrency_reason": concurrency["reason"],
                }
            )
            main_model = provider["defaultModel"]
            main_concurrency = model_concurrency_status(provider, main_model, health)
            model_capacity[main_model] = main_concurrency
            if provider_id in main_workers:
                main_workers[provider_id]["concurrency"] = main_concurrency
                if (
                    main_workers[provider_id]["available"]
                    and not main_concurrency["available"]
                ):
                    main_workers[provider_id]["available"] = False
                    main_workers[provider_id]["reason"] = "concurrency-limit-reached"
        quota = {
            "available": fields["available"],
            "max_used_percent": fields["max_used_percent"],
            "reason": fields["reason"],
        }
        disabled = model in disabled_models or fields.get("disabled", False)
        if quota["available"] and not concurrency["available"] and not disabled:
            fields["available"] = False
            fields["reason"] = "concurrency-limit-reached"
        if quota["available"] and concurrency["available"] and not disabled:
            selected_worker = current_worker
            if "maxConcurrency" in provider:
                selected_worker = {**current_worker, "concurrency": concurrency}
            candidates.append(
                (
                    combined_capacity_priority(quota, concurrency, index),
                    selected_worker,
                )
            )

    if health is not None:
        for model in health:
            provider = provider_for_model(config, model)
            if provider is not None and "maxConcurrency" in provider:
                model_capacity[model] = model_concurrency_status(provider, model, health)

    selected = [item for _, item in sorted(candidates, key=lambda item: item[0])]
    fallback = {"provider": "fallback", **config["fallback"]}
    fallback_active = not selected and fallback["model"] not in disabled_models
    if fallback_active:
        selected = [fallback]
    preferred_main_worker = next(
        (
            main_workers[provider]
            for provider in config["mainProviders"]
            if provider in main_workers and main_workers[provider]["available"]
        ),
        None,
    )
    combined.update(
        {
            "selected_agents": [item["agent"] for item in selected],
            "selected_workers": selected,
            "preferred_worker": selected[0] if selected else None,
            "fallback_active": fallback_active,
            "model_concurrency": model_capacity,
            "main_workers": main_workers,
            "preferred_main_worker": preferred_main_worker,
        }
    )
    combined["orchestration"] = orchestration_contract(combined)
    return combined


def hook_output(
    summary: dict[str, Any], environment: Mapping[str, str] | None = None
) -> dict[str, Any]:
    """Wrap the routing summary in Claude Code's structured hook response."""
    # UserPromptSubmit additionalContext is rendered alongside conversation material by Claude
    # Code. Keep it compact and declarative: imperative orchestration prose here looks like
    # untrusted prompt content after compaction and causes the model to misclassify our own hook.
    advisor_enabled = custom_advisor_enabled(environment)
    metadata = {
        "providers": {},
        "source": "claudex-routing-local-hook",
        "selected_agents": list(summary.get("selected_agents", [])),
        "selected_workers": [
            {
                key: worker[key]
                for key in ("agent", "model", "effort")
                if key in worker
            }
            for worker in summary.get("selected_workers", [])
        ],
        "disabled_subagent_models": list(summary.get("disabled_subagent_models", [])),
        "main_session_model": summary.get("main_session_model"),
        "outer_session_model": summary.get("outer_session_model"),
        "automatic_selection_excluded_models": list(
            summary.get("automatic_selection_excluded_models", [])
        ),
        "sonnet_subagent_suppressed": bool(
            summary.get("sonnet_subagent_suppressed", False)
        ),
        "sonnet_subagent_explicit_allowed": bool(
            summary.get("sonnet_subagent_explicit_allowed", False)
        ),
        "orchestration_mode": summary.get("orchestration_mode", "subagent-first"),
        "delegation_required": bool(summary.get("delegation_required", False)),
        "direct_main_execution": summary.get("direct_main_execution", "allowed"),
        "background_status_required": True,
        "advisor": dict(summary.get("advisor", DEFAULT_ADVISOR)),
        "custom_advisor_enabled": advisor_enabled,
        "custom_advisor_policy": {
            "enabled": advisor_enabled,
            "consult_when": list(CUSTOM_ADVISOR_CONSULT_WHEN),
            "reuse_logical_session": True,
            "not_for_trivial_tasks": True,
        },
        "orchestration": orchestration_contract(summary, environment),
    }
    compact = json.dumps(metadata, ensure_ascii=False, separators=(",", ":"))
    return {
        "hookSpecificOutput": {
            "hookEventName": "UserPromptSubmit",
            "additionalContext": (
                "<system-reminder>\\n"
                "Claudex routing data (runtime metadata; values only):\\n"
                f"{compact}\\n"
                "</system-reminder>"
            ),
        }
    }


def enforce_worker_model_separation(
    summary: dict[str, Any],
    main_model: str | None,
    config: dict[str, Any],
    disabled_models: frozenset[str],
    *,
    outer_model: str | None = None,
    allow_sonnet_subagent: bool | None = None,
) -> dict[str, Any]:
    """Finalize worker routing while conserving a duplicated Sonnet request.

    The outer session and a SubAgent are independent requests, so most providers
    remain selectable when their model is also the current main model.  The
    subscription Sonnet fallback is the deliberate exception: when the outer
    session already runs Sonnet 5, automatic fallback selection would spend an
    additional subscription request for no model diversity.  An explicit
    `CLAUDEX_ALLOW_SONNET_SUBAGENT=1` policy opt-in restores automatic selection;
    direct Agent/Task requests with `claudex_model: claude-sonnet-5` are never
    filtered here and remain available unless the exact model is denylisted.
    """
    separated = json.loads(json.dumps(summary))
    selected = list(separated.get("selected_workers") or [])
    if allow_sonnet_subagent is None:
        allow_sonnet_subagent = _boolean_or_default(
            os.environ, ALLOW_SONNET_SUBAGENT_ENV, False
        )
    session_model = outer_model or main_model
    excluded_models: set[str] = set()
    sonnet_suppressed = False
    if is_sonnet_model(session_model) and not allow_sonnet_subagent:
        retained: list[dict[str, Any]] = []
        for worker_item in selected:
            if is_sonnet_model(worker_item.get("model")):
                model = worker_item.get("model")
                if isinstance(model, str):
                    excluded_models.add(model)
                sonnet_suppressed = True
            else:
                retained.append(worker_item)
        selected = retained
    separated["selected_workers"] = selected
    separated["selected_agents"] = [worker["agent"] for worker in selected]
    separated["preferred_worker"] = selected[0] if selected else None
    separated["main_session_model"] = main_model
    separated["outer_session_model"] = outer_model
    separated["automatic_selection_excluded_models"] = sorted(excluded_models)
    separated["sonnet_subagent_suppressed"] = sonnet_suppressed
    separated["sonnet_subagent_explicit_allowed"] = bool(allow_sonnet_subagent)
    if sonnet_suppressed:
        separated["fallback_active"] = False
    separated["orchestration_mode"] = "subagent-first"
    separated["orchestration"] = orchestration_contract(separated)
    separated["delegation_required"] = bool(selected) and separated["orchestration"].get(
        "subagent_first", True
    )
    separated["direct_main_execution"] = (
        "fallback-only" if separated["delegation_required"] else "allowed"
    )
    separated["background_status_required"] = True
    separated["orchestration"] = orchestration_contract(separated)
    return separated


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
    write_private_json(
        path,
        {"created_at": now, "configuration_key": key, "summary": summary},
    )


def write_private_json(path: Path, value: dict[str, Any]) -> None:
    """Atomically write private JSON with owner-only permissions."""
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = json.dumps(value, separators=(",", ":"))
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
    """Load a valid partial Codexbar report even when some providers fail."""
    completed = subprocess.run(
        [program, "usage", "--json"],
        check=False,
        capture_output=True,
        text=True,
        timeout=USAGE_COMMAND_TIMEOUT_SECONDS,
    )
    return strict_json_array(completed.stdout)


def strict_json_array(output: str) -> list[Any]:
    """Decode exactly one JSON array without exposing rejected output."""
    value = json.loads(output)
    if not isinstance(value, list):
        raise ValueError("Codexbar output must be a JSON array")
    return value


def single_value(values: dict[str, list[str]], key: str) -> str:
    """Read one required query or form value without accepting ambiguity."""
    matches = values.get(key, [])
    if len(matches) != 1 or not matches[0]:
        raise ValueError(f"Qwen request must contain one {key}")
    return matches[0]


def qwen_curl_request(path: Path) -> dict[str, str]:
    """Extract only validated request data from a browser Copy-as-cURL file."""
    tokens = [
        token
        for token in shlex.split(path.read_text(encoding="utf-8"))
        if token.strip() and token != "\\"
    ]
    if not tokens or Path(tokens[0]).name != "curl":
        raise ValueError("Qwen quota input must be a curl command")

    url = ""
    cookie = ""
    body = ""
    content_type = ""
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token.startswith("https://"):
            if url:
                raise ValueError("Qwen curl command must contain one URL")
            url = token
            index += 1
            continue
        option, separator, inline = token.partition("=")
        if option in {"-H", "--header", "-b", "--cookie", "--data", "--data-raw"}:
            value = inline if separator else next_curl_value(tokens, index)
            index += 1 if separator else 2
            if option in {"-H", "--header"}:
                name, header_separator, header_value = value.partition(":")
                if header_separator and name.strip().casefold() == "content-type":
                    content_type = header_value.strip().casefold()
            elif option in {"-b", "--cookie"}:
                cookie = unique_curl_value(cookie, value, "cookie")
            else:
                body = unique_curl_value(body, value, "request body")
            continue
        raise ValueError("Qwen curl command contains an unsupported argument")

    validate_qwen_request(url, cookie, body, content_type)
    return {"url": url, "cookie": cookie, "body": body}


def next_curl_value(tokens: list[str], index: int) -> str:
    """Read the value following a supported curl option."""
    if index + 1 >= len(tokens):
        raise ValueError("Qwen curl option is missing a value")
    return tokens[index + 1]


def unique_curl_value(current: str, value: str, name: str) -> str:
    """Reject duplicate security-sensitive curl values."""
    if current or not value:
        raise ValueError(f"Qwen curl command has an invalid {name}")
    return value


def validate_qwen_request(url: str, cookie: str, body: str, content_type: str) -> None:
    """Allow only Qwen Cloud's personal Token Plan usage request."""
    parsed = urlparse(url)
    if (
        parsed.scheme != "https"
        or parsed.hostname != QWEN_CONSOLE_HOST
        or parsed.path != QWEN_CONSOLE_PATH
        or parsed.port not in {None, 443}
        or parsed.username is not None
        or parsed.fragment
        or any(character in url + cookie + body for character in "\r\n\0")
    ):
        raise ValueError("Qwen curl command targets an unexpected endpoint")
    query = parse_qs(parsed.query, keep_blank_values=True)
    if set(query) != {"product", "action", "api"}:
        raise ValueError("Qwen curl command has unexpected query fields")
    if (
        single_value(query, "product") != QWEN_CONSOLE_PRODUCT
        or single_value(query, "action") != QWEN_CONSOLE_ACTION
        or single_value(query, "api") != QWEN_QUOTA_API
    ):
        raise ValueError("Qwen curl command targets an unexpected API")
    form = parse_qs(body, keep_blank_values=True)
    if set(form) != {"product", "action", "sec_token", "region", "params"}:
        raise ValueError("Qwen curl command has unexpected form fields")
    parameters = json.loads(single_value(form, "params"))
    if (
        single_value(form, "product") != QWEN_CONSOLE_PRODUCT
        or single_value(form, "action") != QWEN_CONSOLE_ACTION
        or not single_value(form, "sec_token")
        or single_value(form, "region") != "ap-southeast-1"
        or not isinstance(parameters, dict)
        or parameters.get("Api") != QWEN_QUOTA_API
        or not isinstance(parameters.get("Data"), dict)
        or parameters.get("V") != "1.0"
        or "=" not in cookie
        or content_type != "application/x-www-form-urlencoded"
    ):
        raise ValueError("Qwen curl command contains invalid request data")


def quota_fraction(value: Any, name: str) -> float:
    """Validate one fractional Qwen quota utilization value."""
    if not valid_percentage(value) or float(value) > 1:
        raise ValueError(f"Qwen quota response contains invalid {name}")
    return float(value)


def quota_reset(value: Any, name: str) -> int:
    """Validate one millisecond reset timestamp."""
    if (
        not isinstance(value, (int, float))
        or isinstance(value, bool)
        or not math.isfinite(value)
        or value < 0
        or not float(value).is_integer()
    ):
        raise ValueError(f"Qwen quota response contains invalid {name}")
    return int(value)


def qwen_quota_entry(payload: Any, provider: str) -> dict[str, Any]:
    """Convert the Qwen Cloud response to a credential-free routing report."""
    try:
        quota = payload["data"]["DataV2"]["data"]["data"]
    except (KeyError, TypeError) as error:
        raise ValueError("Qwen quota response is missing usage data") from error
    if not isinstance(quota, dict):
        raise ValueError("Qwen quota response usage data must be an object")
    windows = [
        qwen_quota_window(
            quota, "five-hour", "per5HourPercentage", "per5HourResetTime"
        ),
        qwen_quota_window(
            quota, "seven-day", "per1WeekPercentage", "per1WeekResetTime"
        ),
    ]
    maximum = max(window["usedPercent"] for window in windows)
    available = maximum < 100
    return {
        "provider": provider,
        "available": available,
        "reason": "available-qwen-cloud-quota" if available else "exhausted",
        "maxUsedPercent": maximum,
        "quotaWindows": windows,
    }


def qwen_quota_window(
    quota: dict[str, Any], name: str, percentage_key: str, reset_key: str
) -> dict[str, Any]:
    """Sanitize one Qwen rolling quota window."""
    used = round(quota_fraction(quota.get(percentage_key), percentage_key) * 100, 6)
    return {
        "name": name,
        "usedPercent": used,
        "remainingPercent": round(100 - used, 6),
        "resetAtMilliseconds": quota_reset(quota.get(reset_key), reset_key),
    }


def run_qwen_quota(program: str, path: Path, provider: str) -> dict[str, Any]:
    """Fetch Qwen quota with a validated, shell-free curl invocation."""
    request = qwen_curl_request(path)
    completed = subprocess.run(
        [
            program,
            "--silent",
            "--show-error",
            "--fail-with-body",
            "--max-time",
            str(QWEN_REQUEST_TIMEOUT_SECONDS),
            request["url"],
            "--header",
            "accept: application/json",
            "--header",
            "content-type: application/x-www-form-urlencoded",
            "--header",
            "origin: https://home.qwencloud.com",
            "--header",
            "referer: https://home.qwencloud.com/billing/subscription/token-plan-individual",
            "--cookie",
            request["cookie"],
            "--data-raw",
            request["body"],
        ],
        check=True,
        capture_output=True,
        text=True,
        timeout=QWEN_REQUEST_TIMEOUT_SECONDS + QWEN_SUBPROCESS_GRACE_SECONDS,
    )
    return qwen_quota_entry(json.loads(completed.stdout), provider)


def qwen_quota_cache_entry(path: Path, now: float) -> dict[str, Any] | None:
    """Return quota acquired less than one hour ago."""
    try:
        cached = json.loads(path.read_text(encoding="utf-8"))
        fetched_at = parse_utc_datetime(cached["fetched_at"])
        entry = cached["entry"]
        age = now - fetched_at
        if (
            0 <= age < QWEN_QUOTA_CACHE_SECONDS
            and isinstance(entry, dict)
            and entry.get("provider") == QWEN_USAGE_PROVIDER
            and entry.get("reason") in {"available-qwen-cloud-quota", "exhausted"}
            and isinstance(entry.get("quotaWindows"), list)
            and explicitly_reported_status(entry)["reason"] != "unknown"
        ):
            return entry
    except (FileNotFoundError, KeyError, TypeError, ValueError, json.JSONDecodeError):
        pass
    return None


def format_utc_datetime(timestamp: float) -> str:
    """Format a Unix timestamp as an explicit UTC cache acquisition time."""
    return (
        datetime.fromtimestamp(timestamp, timezone.utc)
        .isoformat(timespec="microseconds")
        .replace("+00:00", "Z")
    )


def parse_utc_datetime(value: Any) -> float:
    """Parse the UTC ISO 8601 acquisition time stored in the quota cache."""
    if not isinstance(value, str) or not value.endswith("Z"):
        raise ValueError("Qwen quota cache has an invalid acquisition time")
    parsed = datetime.fromisoformat(f"{value[:-1]}+00:00")
    return parsed.timestamp()


def qwen_quota_cache_path(environment: Mapping[str, str]) -> Path:
    """Resolve the private Qwen quota cache under the effective home directory."""
    return (
        Path(environment.get("HOME", str(Path.home())))
        / ".cache/claudex/qwen-quota.json"
    )


def qwen_quota_refresh_due(
    summary: Any, config: dict[str, Any], cache_path: Path, now: float
) -> bool:
    """Prevent a routing cache from outliving the Qwen quota behind it."""
    providers = summary.get("providers") if isinstance(summary, dict) else None
    if not isinstance(providers, dict):
        return False
    qwen_ids = {
        provider["id"]
        for provider in config["providers"]
        if str(provider.get("usageProvider", "")).casefold() == QWEN_USAGE_PROVIDER
    }
    quota_reasons = {"available-qwen-cloud-quota", "exhausted"}
    uses_quota = any(
        isinstance(providers.get(provider_id), dict)
        and providers[provider_id].get("reason") in quota_reasons
        for provider_id in qwen_ids
    )
    return uses_quota and qwen_quota_cache_entry(cache_path, now) is None


def write_qwen_quota_cache(path: Path, entry: dict[str, Any], now: float) -> None:
    """Persist only sanitized Qwen quota fields, never browser credentials."""
    write_private_json(path, {"fetched_at": format_utc_datetime(now), "entry": entry})


def qwen_compatible_configuration(path: Path, model: str) -> tuple[str, str]:
    """Read Qwen Code's existing compatible endpoint and API key."""
    settings = json.loads(path.read_text(encoding="utf-8"))
    providers = settings.get("modelProviders")
    environment = settings.get("env")
    if not isinstance(providers, dict) or not isinstance(environment, dict):
        raise ValueError("Qwen settings are missing model providers or environment")
    candidates = [
        item
        for items in providers.values()
        if isinstance(items, list)
        for item in items
        if isinstance(item, dict) and item.get("id") == model
    ]
    if len(candidates) != 1:
        raise ValueError("Qwen settings must contain one configured model")
    candidate = candidates[0]
    base_url = candidate.get("baseUrl")
    environment_key = candidate.get("envKey")
    api_key = (
        environment.get(environment_key) if isinstance(environment_key, str) else None
    )
    if not isinstance(base_url, str) or not isinstance(api_key, str) or not api_key:
        raise ValueError("Qwen settings are missing compatible API credentials")
    parsed = urlparse(base_url)
    if (
        parsed.scheme != "https"
        or parsed.hostname is None
        or not parsed.hostname.endswith(".maas.aliyuncs.com")
        or not parsed.hostname.startswith("token-plan.")
        or parsed.path.rstrip("/") != "/compatible-mode/v1"
        or parsed.port not in {None, 443}
        or parsed.params
        or parsed.query
        or parsed.fragment
        or parsed.username is not None
    ):
        raise ValueError("Qwen settings contain an unexpected compatible endpoint")
    return f"{base_url.rstrip('/')}/models", api_key


def qwen_compatible_available(program: str, settings: Path, model: str) -> bool:
    """Verify the configured plan through its non-generative models endpoint."""
    endpoint, api_key = qwen_compatible_configuration(settings, model)
    subprocess.run(
        [
            program,
            "--silent",
            "--show-error",
            "--fail",
            "--output",
            os.devnull,
            "--max-time",
            str(QWEN_REQUEST_TIMEOUT_SECONDS),
            "--header",
            f"Authorization: Bearer {api_key}",
            endpoint,
        ],
        check=True,
        capture_output=True,
        text=True,
        timeout=QWEN_REQUEST_TIMEOUT_SECONDS + QWEN_SUBPROCESS_GRACE_SECONDS,
    )
    return True


def qwen_usage_entry(
    program: str,
    provider: str,
    model: str,
    curl_path: Path,
    settings_path: Path,
    cache_path: Path,
    now: float,
) -> dict[str, Any]:
    """Use fresh quota, refresh stale quota, or verify compatible API availability."""
    if cached := qwen_quota_cache_entry(cache_path, now):
        return cached
    try:
        entry = run_qwen_quota(program, curl_path, provider)
        write_qwen_quota_cache(cache_path, entry, now)
        return entry
    except (
        OSError,
        TypeError,
        ValueError,
        subprocess.SubprocessError,
        json.JSONDecodeError,
    ):
        try:
            qwen_compatible_available(program, settings_path, model)
            return {
                "provider": provider,
                "available": True,
                "reason": "available-compatible-api-only",
            }
        except (
            OSError,
            TypeError,
            ValueError,
            subprocess.SubprocessError,
            json.JSONDecodeError,
        ):
            return unavailable_usage_entry(provider)


def unavailable_usage_entry(provider: str) -> dict[str, Any]:
    """Represent a provider-specific usage command failure."""
    return {
        "provider": provider,
        "available": False,
        "reason": "usage-unavailable",
    }


def ollama_usage_entry(
    curl_program: str,
    provider: str,
    model: str,
    environment: Mapping[str, str],
) -> dict[str, Any]:
    """Keep an Ollama model routable when Codexbar usage is unavailable."""
    base_url = environment.get(OLLAMA_BASE_URL_ENV, DEFAULT_OLLAMA_BASE_URL).rstrip("/")
    parsed = urlparse(base_url)
    if parsed.scheme not in {"http", "https"} or not parsed.netloc:
        return unavailable_usage_entry(provider)
    try:
        completed = subprocess.run(
            [
                curl_program,
                "--fail",
                "--silent",
                "--show-error",
                "--max-time",
                str(QWEN_REQUEST_TIMEOUT_SECONDS),
                f"{base_url}/api/tags",
            ],
            check=True,
            capture_output=True,
            text=True,
            timeout=QWEN_REQUEST_TIMEOUT_SECONDS + QWEN_SUBPROCESS_GRACE_SECONDS,
        )
        payload = json.loads(completed.stdout)
        models = payload.get("models") if isinstance(payload, dict) else None
        if not isinstance(models, list) or not any(
            isinstance(item, dict)
            and model in {item.get("name"), item.get("model")}
            for item in models
        ):
            return unavailable_usage_entry(provider)
        return {
            "provider": provider,
            "available": True,
            "reason": "available-ollama-api-only",
        }
    except (
        OSError,
        ValueError,
        subprocess.SubprocessError,
        json.JSONDecodeError,
    ):
        return unavailable_usage_entry(provider)


def collect_codexbar_report(
    codexbar_program: str,
    codexbar_names: set[str],
    qwen_names: set[str],
) -> list[dict[str, Any]]:
    """Load Codexbar usage, isolating failures from other providers."""
    try:
        raw_report = run_codexbar(codexbar_program)
        if not isinstance(raw_report, list):
            return []
        return [
            entry
            for entry in raw_report
            if not isinstance(entry, dict)
            or str(entry.get("provider", "")).casefold() not in qwen_names
        ]
    except (OSError, ValueError, subprocess.SubprocessError, json.JSONDecodeError):
        return [unavailable_usage_entry(name) for name in sorted(codexbar_names)]


def collect_usage(
    config: dict[str, Any],
    codexbar_program: str,
    curl_program: str,
    environment: Mapping[str, str] | None = None,
    now: float | None = None,
    disabled_models: frozenset[str] | None = None,
) -> list[dict[str, Any]]:
    """Collect independent provider usage and keep failures isolated."""
    env: Mapping[str, str] = os.environ if environment is None else environment
    disabled = disabled_models or frozenset()
    now = time.time() if now is None else now
    home = Path(env.get("HOME", str(Path.home())))
    curl_path = Path(
        env.get("CLAUDEX_QWEN_QUOTA_CURL_FILE", str(DEFAULT_QWEN_CURL))
    ).expanduser()
    settings_path = Path(
        env.get("CLAUDEX_QWEN_SETTINGS_FILE", str(home / ".qwen/settings.json"))
    ).expanduser()
    quota_cache_path = qwen_quota_cache_path(env)
    providers = [
        provider
        for provider in config["providers"]
        if worker(provider)["model"] not in disabled
    ]
    if not providers:
        return [
            unavailable_usage_entry(str(provider["id"]))
            for provider in config["providers"]
        ]
    qwen_providers = [
        provider
        for provider in providers
        if str(provider.get("usageProvider", "")).casefold() == QWEN_USAGE_PROVIDER
    ]
    qwen_names = {provider["usageProvider"].casefold() for provider in qwen_providers}
    ollama_providers = [
        provider
        for provider in providers
        if str(provider.get("usageProvider", "")).casefold() == OLLAMA_USAGE_PROVIDER
    ]
    codexbar_names = {
        provider["usageProvider"]
        for provider in providers
        if isinstance(provider.get("usageProvider"), str)
        and provider["usageProvider"]
        and provider not in qwen_providers
    }
    # Codexbar and Qwen quota are independent; run them together so a cold hook
    # pays max(source latency) instead of sum(source latency). Ollama's API is
    # only a fallback for a missing or unusable Codexbar entry, so do not start
    # an otherwise unnecessary request that can hold the hook on executor exit.
    worker_count = 1 + len(qwen_providers)
    with ThreadPoolExecutor(max_workers=worker_count) as pool:
        codexbar_future = pool.submit(
            collect_codexbar_report, codexbar_program, codexbar_names, qwen_names
        )
        qwen_futures = [
            pool.submit(
                qwen_usage_entry,
                curl_program,
                provider["usageProvider"],
                provider["defaultModel"],
                curl_path,
                settings_path,
                quota_cache_path,
                now,
            )
            for provider in qwen_providers
        ]
        report = list(codexbar_future.result())
        report.extend(future.result() for future in qwen_futures)
    fallback_providers = [
        provider
        for provider in ollama_providers
        if provider_status(report, provider["usageProvider"])["reason"]
        in {"missing", "unknown", "usage-unavailable"}
    ]
    if fallback_providers:
        # Multiple missing Ollama providers are independent. Keep their
        # fallback probes parallel, but never delay a valid Codexbar result for
        # an API probe whose result will be discarded.
        with ThreadPoolExecutor(max_workers=len(fallback_providers)) as pool:
            fallback_futures = [
                (
                    provider,
                    pool.submit(
                        ollama_usage_entry,
                        curl_program,
                        provider["usageProvider"],
                        worker(provider)["model"],
                        env,
                    ),
                )
                for provider in fallback_providers
            ]
            for provider, future in fallback_futures:
                usage_provider = provider["usageProvider"]
                report = [
                    entry
                    for entry in report
                    if not isinstance(entry, dict)
                    or str(entry.get("provider", "")).casefold()
                    != usage_provider.casefold()
                ]
                report.append(future.result())
    return report


def cache_seconds(environment: Mapping[str, str]) -> int:
    """Parse the optional cache TTL, falling back safely on invalid values."""
    try:
        return max(
            0,
            int(environment.get("CLAUDEX_USAGE_CACHE_SECONDS", DEFAULT_CACHE_SECONDS)),
        )
    except ValueError:
        return DEFAULT_CACHE_SECONDS


def fallback_summary(
    reason: str,
    config: dict[str, Any] | None = None,
    disabled_models: frozenset[str] = frozenset(),
) -> dict[str, Any]:
    """Prefer the configured native fallback when usage cannot be established."""
    config = config or load_config(config_path(os.environ))
    providers = {}
    for provider in config["providers"]:
        disabled = worker(provider)["model"] in disabled_models
        unavailable_reason = "disabled-by-policy" if disabled else reason
        providers[provider["id"]] = {
            **status(False, None, unavailable_reason),
            **worker(provider),
            "disabled": disabled,
        }
    fallback = {"provider": "fallback", **config["fallback"]}
    selected = [] if fallback["model"] in disabled_models else [fallback]
    summary = {
        "providers": providers,
        "main_workers": {},
        "selected_agents": [item["agent"] for item in selected],
        "selected_workers": selected,
        "preferred_worker": selected[0] if selected else None,
        "fallback_active": bool(selected),
        "preferred_main_worker": None,
        "disabled_subagent_models": sorted(disabled_models),
        "advisor": dict(config.get("advisor", DEFAULT_ADVISOR)),
    }
    summary["orchestration"] = orchestration_contract(summary)
    return summary


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, help="provider routing JSON")
    parser.add_argument(
        "--disabled-models-config", type=Path, help="disabled SubAgent models JSON"
    )
    parser.add_argument("--input", type=Path, help="read a Codexbar JSON fixture")
    parser.add_argument("--no-cache", action="store_true")
    parser.add_argument("--codexbar-program", default="codexbar")
    parser.add_argument("--curl-program", default="curl")
    return parser.parse_args()


def main() -> int:
    arguments = parse_arguments()
    now = time.time()
    try:
        config = load_config(config_path(os.environ, arguments.config))
        orchestration_settings(os.environ)
        explicit_disabled_config = (
            arguments.disabled_models_config is not None
            or DISABLED_SUBAGENT_MODELS_CONFIG_ENV in os.environ
        )
        if not explicit_disabled_config and RESOLVED_DISABLED_SUBAGENT_MODELS_ENV in os.environ:
            disabled_models = environment_models(
                os.environ, RESOLVED_DISABLED_SUBAGENT_MODELS_ENV
            )
        else:
            configured_disabled_models = load_disabled_models_config(
                disabled_models_config_path(
                    os.environ, arguments.disabled_models_config
                )
            )
            disabled_models = disabled_subagent_models(
                configured_disabled_models, os.environ
            )
    except (OSError, ValueError, json.JSONDecodeError) as error:
        raise SystemExit(f"claudex routing configuration error: {error}") from error
    key = configuration_key(config, disabled_models)
    cache_path = Path.home() / ".cache/claudex/usage-routing.json"
    quota_cache_path = qwen_quota_cache_path(os.environ)
    ttl = 0 if arguments.no_cache or arguments.input else cache_seconds(os.environ)
    summary = read_cache(cache_path, now, ttl, key)
    if summary is not None and qwen_quota_refresh_due(
        summary, config, quota_cache_path, now
    ):
        summary = None
    if summary is None:
        try:
            report = (
                json.loads(arguments.input.read_text(encoding="utf-8"))
                if arguments.input
                else collect_usage(
                    config,
                    arguments.codexbar_program,
                    arguments.curl_program,
                    os.environ,
                    now,
                    disabled_models,
                )
            )
            summary = routing_summary(report, config, disabled_models)
            if ttl > 0:
                write_cache(cache_path, summary, now, key)
        except (OSError, ValueError, subprocess.SubprocessError, json.JSONDecodeError):
            summary = fallback_summary("usage-unavailable", config, disabled_models)
    summary = apply_model_concurrency(
        summary,
        config,
        run_daemon_health(arguments.curl_program, os.environ),
        disabled_models,
    )
    summary = enforce_worker_model_separation(
        summary,
        os.environ.get("CLAUDEX_MAIN_MODEL"),
        config,
        disabled_models,
        outer_model=os.environ.get(OUTER_MODEL_ENV),
        allow_sonnet_subagent=_boolean_or_default(
            os.environ, ALLOW_SONNET_SUBAGENT_ENV, False
        ),
    )
    print(
        json.dumps(
            hook_output(summary, os.environ), ensure_ascii=False, separators=(",", ":")
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
