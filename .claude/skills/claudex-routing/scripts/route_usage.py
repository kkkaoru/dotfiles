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
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from urllib.parse import parse_qs, urlparse

DEFAULT_CACHE_SECONDS = 300
ROUTING_CACHE_VERSION = 2
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
QWEN_CONSOLE_HOST = "cs-data.qwencloud.com"
QWEN_CONSOLE_PATH = "/data/api.json"
QWEN_CONSOLE_PRODUCT = "sfm_bailian"
QWEN_CONSOLE_ACTION = "IntlBroadScopeAspnGateway"
QWEN_QUOTA_API = "zeldaHttp.apikeyMgr./tokenplan/personal/api/v2/usage"
USAGE_COMMAND_TIMEOUT_SECONDS = 45
DISABLED_SUBAGENT_MODELS_ENV = "CLAUDEX_DISABLED_SUBAGENT_MODELS"
DISABLED_SUBAGENT_MODELS_CONFIG_ENV = "CLAUDEX_DISABLED_SUBAGENT_MODELS_CONFIG"
RESOLVED_DISABLED_SUBAGENT_MODELS_ENV = "CLAUDEX_RESOLVED_DISABLED_SUBAGENT_MODELS"


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


def disabled_models_config_path(
    environment: dict[str, str], requested: Path | None = None
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
    return isinstance(provider, dict) and all(
        isinstance(provider.get(field), str) and provider[field] for field in required
    )


def valid_choice(choice: Any) -> bool:
    """Check the native fallback agent selection."""
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
    configured: frozenset[str], environment: dict[str, str]
) -> frozenset[str]:
    """Merge configured models with the terminal-local exact model denylist."""
    return configured | environment_models(environment, DISABLED_SUBAGENT_MODELS_ENV)


def environment_models(environment: dict[str, str], name: str) -> frozenset[str]:
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
    report: Any,
    config: dict[str, Any] | None = None,
    disabled_models: frozenset[str] = frozenset(),
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
        disabled = provider["defaultModel"] in disabled_models
        effective = status(False, None, "disabled-by-policy") if disabled else quota
        providers[provider["id"]] = {**effective, **worker(provider), "disabled": disabled}
        if quota["available"] and not disabled:
            candidates.append((capacity_priority(quota, index), worker(provider)))
    selected = [
        item for _, item in sorted(candidates, key=lambda candidate: candidate[0])
    ]
    fallback_active = not selected and config["fallback"]["model"] not in disabled_models
    if fallback_active:
        selected = [{"provider": "fallback", **config["fallback"]}]
    return {
        "providers": providers,
        "selected_agents": [item["agent"] for item in selected],
        "selected_workers": selected,
        "preferred_worker": selected[0] if selected else None,
        "fallback_active": fallback_active,
        "disabled_subagent_models": sorted(disabled_models),
    }


def hook_output(summary: dict[str, Any]) -> dict[str, Any]:
    """Wrap the routing summary in Claude Code's structured hook response."""
    compact = json.dumps(summary, ensure_ascii=False, separators=(",", ":"))
    instructions = (
        " Follow claudex-routing: delegation is the standing default for substantive work unless "
        "the user opts out; do not wait for them to repeat it or merely announce future delegation. "
        "Use selected_workers and pass each "
        "worker's model and effort as claudex_model and claudex_effort for every Agent/Task launch, "
        "preserve the main session's complete tool set and permission context, and never add an "
        "implicit read-only, plan-only, no-edit, no-build, or no-deploy restriction; use foreground "
        "delegation when background execution would auto-deny an interactive main-session permission. "
        "prefer preferred_worker for primary work because known quota headroom orders the list, "
        "including nested launches from a worker; never default a nested launch to generic claude "
        "or blindly inherit its parent route. If the user names a "
        "model matching model_prefixes, dynamically select that provider and pass the exact "
        "requested model only when it is not in disabled_subagent_models. The merged configured "
        "and terminal-local disabled_subagent_models list is an absolute SubAgent denylist: never "
        "launch, inherit, "
        "dynamically select, or reuse one of those exact models, even when the user names it. If "
        "selected_workers is empty, continue safely in the main session and report that no "
        "allowed SubAgent model is available. This current routing context overrides stale "
        "auto-memory about worker "
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
    """Load Codexbar JSON without involving a shell."""
    completed = subprocess.run(
        [program, "usage", "--json"],
        check=True,
        capture_output=True,
        text=True,
        timeout=USAGE_COMMAND_TIMEOUT_SECONDS,
    )
    return json.loads(completed.stdout)


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


def qwen_quota_cache_path(environment: dict[str, str]) -> Path:
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


def collect_usage(
    config: dict[str, Any],
    codexbar_program: str,
    curl_program: str,
    environment: dict[str, str] | None = None,
    now: float | None = None,
) -> list[dict[str, Any]]:
    """Collect independent provider usage and keep failures isolated."""
    environment = os.environ if environment is None else environment
    now = time.time() if now is None else now
    home = Path(environment.get("HOME", str(Path.home())))
    curl_path = Path(
        environment.get("CLAUDEX_QWEN_QUOTA_CURL_FILE", str(DEFAULT_QWEN_CURL))
    ).expanduser()
    settings_path = Path(
        environment.get("CLAUDEX_QWEN_SETTINGS_FILE", str(home / ".qwen/settings.json"))
    ).expanduser()
    quota_cache_path = qwen_quota_cache_path(environment)
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
        report.append(
            qwen_usage_entry(
                curl_program,
                provider["usageProvider"],
                provider["defaultModel"],
                curl_path,
                settings_path,
                quota_cache_path,
                now,
            )
        )
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
    reason: str,
    config: dict[str, Any] | None = None,
    disabled_models: frozenset[str] = frozenset(),
) -> dict[str, Any]:
    """Prefer the configured native fallback when usage cannot be established."""
    config = config or load_config(config_path(os.environ))
    providers = {}
    for provider in config["providers"]:
        disabled = provider["defaultModel"] in disabled_models
        unavailable_reason = "disabled-by-policy" if disabled else reason
        providers[provider["id"]] = {
            **status(False, None, unavailable_reason),
            **worker(provider),
            "disabled": disabled,
        }
    fallback = {"provider": "fallback", **config["fallback"]}
    selected = [] if fallback["model"] in disabled_models else [fallback]
    return {
        "providers": providers,
        "selected_agents": [item["agent"] for item in selected],
        "selected_workers": selected,
        "preferred_worker": selected[0] if selected else None,
        "fallback_active": bool(selected),
        "disabled_subagent_models": sorted(disabled_models),
    }


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
        if RESOLVED_DISABLED_SUBAGENT_MODELS_ENV in os.environ:
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
                )
            )
            summary = routing_summary(report, config, disabled_models)
            if ttl > 0:
                write_cache(cache_path, summary, now, key)
        except (OSError, ValueError, subprocess.SubprocessError, json.JSONDecodeError):
            summary = fallback_summary("usage-unavailable", config, disabled_models)
    print(json.dumps(hook_output(summary), ensure_ascii=False, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
