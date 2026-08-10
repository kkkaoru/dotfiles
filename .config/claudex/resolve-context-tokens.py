#!/usr/bin/env python3

"""Resolve Claude Code's assumed context window for an outer claudex model.

Unrecognized Claude Code model ids default to 200k auto-compact. Provider
``maxContextTokens`` is the real window for Codex/selectable models such as
``gpt-5.6-terra`` and Cursor ``auto``. Print that integer, or nothing when the
model is native Claude / has no configured window.
"""

from __future__ import annotations

import json
from pathlib import Path
import sys
from typing import Any


DISCOVERY_PREFIX = "claude-claudex-"


def load_json_object(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path} must be a JSON object")
    return value


def canonical_model(model: str) -> str:
    if model.startswith(DISCOVERY_PREFIX):
        return model[len(DISCOVERY_PREFIX) :]
    return model


def match_key(provider: dict[str, Any], model: str) -> tuple[int, int]:
    if provider.get("enabled") is False:
        return (0, 0)
    default = provider.get("defaultModel")
    subagent = provider.get("subagentModel")
    selectable = provider.get("selectableModels") or []
    if model == default or model == subagent or model in selectable:
        return (2, 0)
    prefixes = [
        prefix
        for prefix in provider.get("modelPrefixes") or []
        if isinstance(prefix, str) and prefix and model.startswith(prefix)
    ]
    if prefixes:
        return (1, max(len(prefix) for prefix in prefixes))
    return (0, 0)


def context_tokens_for_model(config: dict[str, Any], model: str) -> int | None:
    model = canonical_model(model.strip())
    if not model:
        return None
    best_tokens: int | None = None
    best_key = (0, 0)
    providers = config.get("providers") or []
    if not isinstance(providers, list):
        raise ValueError("providers must be an array")
    for provider in providers:
        if not isinstance(provider, dict):
            continue
        key = match_key(provider, model)
        if key[0] == 0:
            continue
        tokens = provider.get("maxContextTokens")
        if not isinstance(tokens, int) or tokens <= 0:
            continue
        if key > best_key:
            best_key = key
            best_tokens = tokens
    return best_tokens


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit("usage: resolve-context-tokens.py <providers-json> <model>")
    config_path = Path(sys.argv[1])
    model = sys.argv[2]
    if not model or any(char in model for char in "\r\n"):
        raise ValueError("model must be a non-empty single-line value")
    tokens = context_tokens_for_model(load_json_object(config_path), model)
    if tokens is not None:
        print(tokens)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"claudex: {error}", file=sys.stderr)
        raise SystemExit(2) from error
