#!/usr/bin/env python3

"""Resolve the claudex outer-session model and effort defaults.

The result is deliberately line-oriented so the fish launcher can consume it
without evaluating user-provided configuration as shell code.  The output is:

    source
    effective_model
    effective_effort
    settings_model
    settings_effort

``settings_model`` and ``settings_effort`` are equal to the effective values in
``settings`` mode and are empty in ``explicit`` mode.  A model supplied through
the legacy ``CLAUDEX_MODEL`` environment variable selects ``explicit`` mode so
the existing one-shot override remains backwards compatible.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
import sys
from typing import Any


DEFAULT_MODEL = "opus"
DEFAULT_EFFORT = "medium"
VALID_SOURCES = {"explicit", "settings"}
VALID_EFFORTS = {"low", "medium", "mid", "high", "xhigh", "max"}


def load_optional(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {}
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read JSON file {path}: {error}") from error
    if not isinstance(value, dict):
        raise ValueError(f"{path} must be a JSON object")
    return value


def required_string(value: Any, name: str, path: Path) -> str:
    if not isinstance(value, str) or not value or any(char in value for char in "\r\n"):
        raise ValueError(f"{path} field {name} must be a non-empty single-line string")
    return value


def effort(value: Any, name: str, path: Path) -> str:
    value = required_string(value, name, path)
    if value not in VALID_EFFORTS:
        allowed = ", ".join(sorted(VALID_EFFORTS))
        raise ValueError(f"{path} field {name} must be one of: {allowed}")
    return value


def env_value(name: str) -> str | None:
    # Distinguish an unset variable from an explicitly empty override.  Empty
    # overrides are rejected instead of silently falling back to a different
    # model, which makes one-shot shell configuration failures obvious.
    if name not in os.environ:
        return None
    value = os.environ[name]
    if not value or any(char in value for char in "\r\n"):
        raise ValueError(f"{name} must be a non-empty single-line value")
    return value


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit("usage: resolve-defaults.py <defaults-json-or-> <settings-json>")

    defaults_path = Path(sys.argv[1])
    settings_path = Path(sys.argv[2])
    defaults = load_optional(defaults_path) if sys.argv[1] != "-" else {}

    version = defaults.get("version")
    if version is not None and version != 1:
        raise ValueError(f"{defaults_path} field version must be 1")

    configured_source = defaults.get("source")
    if configured_source is None:
        # A present local defaults file owns the outer model/effort for this
        # machine. Default to explicit so `model`/`effort` apply without an
        # easy-to-miss source field. Settings inheritance stays available via
        # `"source": "settings"`.
        configured_source = "explicit" if defaults else "settings"
    if not isinstance(configured_source, str):
        raise ValueError(f"{defaults_path} field source must be `explicit` or `settings`")

    source_override = os.environ.get("CLAUDEX_DEFAULTS_SOURCE")
    source = source_override if source_override is not None else configured_source
    if source not in VALID_SOURCES:
        raise ValueError(
            "CLAUDEX_DEFAULTS_SOURCE or defaults.local.json source must be `explicit` or `settings`"
        )

    configured_model = defaults.get("model", DEFAULT_MODEL)
    configured_model = required_string(configured_model, "model", defaults_path)
    configured_effort = defaults.get("effort", DEFAULT_EFFORT)
    configured_effort = effort(configured_effort, "effort", defaults_path)

    model_override = env_value("CLAUDEX_MODEL")
    effort_override = env_value("CLAUDEX_EFFORT")
    if model_override is not None:
        # CLAUDEX_MODEL historically selected the outer model.  Preserve that
        # contract even when a persistent settings source is configured.
        source = "explicit"

    if source == "settings":
        settings = load_optional(settings_path)
        settings_model = required_string(settings.get("model"), "model", settings_path)
        settings_effort = effort(settings.get("effortLevel"), "effortLevel", settings_path)
        # Optional per-machine overlays on top of shared Claude settings.
        if "model" in defaults:
            settings_model = required_string(defaults.get("model"), "model", defaults_path)
        if "effort" in defaults:
            settings_effort = effort(defaults.get("effort"), "effort", defaults_path)
        effective_model = settings_model
        effective_effort = effort_override or settings_effort
        output_settings = (settings_model, settings_effort)
    else:
        effective_model = model_override or configured_model
        effective_effort = effort_override or configured_effort
        output_settings = ("", "")

    print(source)
    print(effective_model)
    print(effective_effort)
    print(output_settings[0])
    print(output_settings[1])
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ValueError as error:
        print(f"claudex: {error}", file=sys.stderr)
        raise SystemExit(2) from error
