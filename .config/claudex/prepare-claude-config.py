#!/usr/bin/env python3

"""Prepare an isolated Claude Code config dir for claudex.

Plain `claude` keeps using ``$HOME/.claude/settings.json``. Claudex sets
``CLAUDE_CONFIG_DIR`` to a sibling tree so ``/model`` and outer defaults cannot
overwrite the shared Claude Code model. Shared resources (agents, sessions,
history, hooks, …) are symlinked from the user Claude directory.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
import sys
from typing import Any


SHARED_SETTINGS_NAMES = frozenset({"settings.json", "settings.local.json"})
DISCOVERY_PREFIX = "claude-claudex-"
PLAIN_CLAUDE_FALLBACK_MODEL = "sonnet[1m]"
DEFAULT_STOP_HOOK_BLOCK_CAP = "64"
DEFAULT_SUBAGENT_PROMPT_CACHE_TTL = "5m"
# xAI publishes a 500k window for grok-4.6; the launcher still sends that via
# CLAUDE_CODE_MAX_CONTEXT_TOKENS from providers.json.
TOOL_POLICY_HOOK = (
    'test "${CLAUDEX_ACTIVE:-}" != 1 || '
    'exec "$HOME/.cargo/bin/claudex-tool-policy"'
)
BACKGROUND_BASH_HOOK = (
    'test "${CLAUDEX_ACTIVE:-}" != 1 || '
    'exec "$HOME/.pi/agent/extensions/tmux-timeout/claudex-hook.ts"'
)
BACKGROUND_COMPLETION_HOOK = (
    'test "${CLAUDEX_ACTIVE:-}" != 1 || '
    'exec "$HOME/.pi/agent/extensions/tmux-timeout/claudex-completion-hook.ts"'
)
CLAUDEX_MANAGED_HOOK_COMMANDS = (
    "claudex-tool-policy",
    "claudex-hook.ts",
    "claudex-completion-hook.ts",
)
# External model ids accepted by the Claudex adapter but not shipped in Claude
# Code's built-in catalog. Identity mappings suppress the SDK warning without
# changing the model sent to the adapter.
CLAUDEX_EXTERNAL_MODEL_IDS = (
    "grok-4.6",
    "cursor/gpt-5.6-luna",
    "cursor/gpt-5.6-sol",
    "cursor/gpt-5.6-terra",
)
DISABLED_SUBAGENT_MODELS_NAME = "disabled-subagent-models.json"
ISOLATED_CONFIG_DIR_NAME = "claude-config"
EMPTY_DISABLED_SUBAGENT_MODELS: dict[str, object] = {
    "version": 1,
    "disabledModels": [],
}
# Registered only on the claudex-isolated settings.json. Plain `claude` keeps
# using ~/.claude/settings.json without these mechanical tool limits.
CLAUDEX_TOOL_POLICY_HOOKS: dict[str, list[dict[str, Any]]] = {
    "PreToolUse": [
        {
            "matcher": "Bash",
            "hooks": [{"type": "command", "command": BACKGROUND_BASH_HOOK, "timeout": 10}],
        },
        {
            "matcher": "Read|Write|Edit|MultiEdit|NotebookEdit|Grep|Glob|LS|WebSearch|WebFetch",
            "hooks": [{"type": "command", "command": TOOL_POLICY_HOOK, "timeout": 10}],
        }
    ],
    "PostToolUse": [
        {
            "matcher": "Bash",
            "hooks": [
                {
                    "type": "command",
                    "command": BACKGROUND_COMPLETION_HOOK,
                    "asyncRewake": True,
                    "timeout": 604800,
                }
            ],
        },
        {
            "matcher": "Write|Edit|MultiEdit|NotebookEdit",
            "hooks": [{"type": "command", "command": TOOL_POLICY_HOOK, "timeout": 10}],
        },
    ],
    "SubagentStop": [
        {
            "matcher": "*",
            "hooks": [{"type": "command", "command": TOOL_POLICY_HOOK, "timeout": 10}],
        }
    ],
    "SessionEnd": [
        {
            "matcher": "*",
            "hooks": [{"type": "command", "command": TOOL_POLICY_HOOK, "timeout": 10}],
        }
    ],
}


def load_json_object(path: Path) -> dict[str, Any]:
    if not path.is_file():
        return {}
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read JSON file {path}: {error}") from error
    if not isinstance(value, dict):
        raise ValueError(f"{path} must be a JSON object")
    return value


def required_single_line(value: str, name: str) -> str:
    if not value or any(char in value for char in "\r\n"):
        raise ValueError(f"{name} must be a non-empty single-line value")
    return value


def is_claudex_discovery_model(model: str) -> bool:
    return model.startswith(DISCOVERY_PREFIX)


def plain_claude_model(model: str) -> str:
    # Shared settings must never keep gateway discovery aliases that plain
    # Claude Code cannot route without the claudex adapter.
    if is_claudex_discovery_model(model):
        return PLAIN_CLAUDE_FALLBACK_MODEL
    return model


def write_empty_disabled_subagent_models(path: Path) -> None:
    path.write_text(
        json.dumps(EMPTY_DISABLED_SUBAGENT_MODELS, indent=2) + "\n",
        encoding="utf-8",
    )


def ensure_disabled_subagent_models(path: Path) -> None:
    if path.is_file():
        if path.read_text(encoding="utf-8").strip():
            return
    elif path.exists():
        raise ValueError(f"{path} exists and is not a file")
    path.parent.mkdir(parents=True, exist_ok=True)
    source = Path(__file__).resolve().parent / DISABLED_SUBAGENT_MODELS_NAME
    if source.is_file() and source.resolve() != path.resolve():
        path.write_text(source.read_text(encoding="utf-8"), encoding="utf-8")
        return
    write_empty_disabled_subagent_models(path)


def ensure_isolated_denylist(isolated: Path) -> None:
    if isolated.name != ISOLATED_CONFIG_DIR_NAME:
        return
    ensure_disabled_subagent_models(isolated.parent / DISABLED_SUBAGENT_MODELS_NAME)


def ensure_symlink(target: Path, link: Path) -> None:
    if link.exists() or link.is_symlink():
        if link.is_symlink() and link.resolve() == target.resolve():
            return
        if link.is_symlink() or link.is_file():
            link.unlink()
        else:
            return
    link.symlink_to(target)


def mirror_shared_entries(user_claude: Path, isolated: Path) -> None:
    isolated.mkdir(parents=True, exist_ok=True)
    if not user_claude.is_dir():
        return
    for entry in user_claude.iterdir():
        if entry.name in SHARED_SETTINGS_NAMES:
            continue
        ensure_symlink(entry, isolated / entry.name)


def merge_claudex_tool_policy_hooks(settings: dict[str, Any]) -> None:
    hooks = settings.get("hooks")
    if not isinstance(hooks, dict):
        hooks = {}
        settings["hooks"] = hooks
    for event_name, entries in CLAUDEX_TOOL_POLICY_HOOKS.items():
        existing = hooks.get(event_name)
        merged: list[Any] = list(existing) if isinstance(existing, list) else []
        # Drop prior copies of this policy so repeated prepare stays idempotent.
        retained = [
            entry
            for entry in merged
            if not (
                isinstance(entry, dict)
                and isinstance(entry.get("hooks"), list)
                and any(
                    isinstance(hook, dict)
                    and any(
                        marker in str(hook.get("command", ""))
                        for marker in CLAUDEX_MANAGED_HOOK_COMMANDS
                    )
                    for hook in entry["hooks"]
                )
            )
        ]
        hooks[event_name] = retained + entries


def apply_context_token_env(settings: dict[str, Any], context_tokens: str) -> None:
    env = settings.get("env")
    if not isinstance(env, dict):
        env = {}
        settings["env"] = env
    if context_tokens:
        env["CLAUDE_CODE_MAX_CONTEXT_TOKENS"] = context_tokens
    else:
        env.pop("CLAUDE_CODE_MAX_CONTEXT_TOKENS", None)


def apply_claudex_hook_env(settings: dict[str, Any]) -> None:
    env = settings.get("env")
    if not isinstance(env, dict):
        env = {}
        settings["env"] = env
    # A caller can still choose a different finite guard in its environment or
    # shared Claude settings.  The isolated claudex default raises only the
    # built-in repeated Stop-hook safety threshold used by long `/goal` runs.
    env.setdefault("CLAUDE_CODE_STOP_HOOK_BLOCK_CAP", DEFAULT_STOP_HOOK_BLOCK_CAP)


def apply_subagent_prompt_cache_ttl(settings: dict[str, Any]) -> None:
    settings.setdefault("subagentPromptCacheTtl", DEFAULT_SUBAGENT_PROMPT_CACHE_TTL)


def apply_external_model_overrides(settings: dict[str, Any]) -> None:
    overrides = settings.get("modelOverrides")
    if not isinstance(overrides, dict):
        overrides = {}
        settings["modelOverrides"] = overrides
    for model in CLAUDEX_EXTERNAL_MODEL_IDS:
        overrides.setdefault(model, model)
    # Drop leftover grok-4.5 identity keys copied from older user settings.
    overrides.pop("grok-4.5", None)


def write_isolated_settings(
    user_settings_path: Path,
    isolated_settings_path: Path,
    model: str,
    effort: str,
    context_tokens: str = "",
) -> None:
    settings = load_json_object(user_settings_path)
    settings["model"] = model
    settings["effortLevel"] = effort
    apply_context_token_env(settings, context_tokens)
    apply_claudex_hook_env(settings)
    apply_subagent_prompt_cache_ttl(settings)
    apply_external_model_overrides(settings)
    merge_claudex_tool_policy_hooks(settings)
    isolated_settings_path.write_text(
        json.dumps(settings, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )


def sanitize_shared_settings(user_settings_path: Path) -> None:
    if not user_settings_path.is_file():
        return
    settings = load_json_object(user_settings_path)
    model = settings.get("model")
    if not isinstance(model, str) or not model:
        return
    cleaned = plain_claude_model(model)
    if cleaned == model:
        return
    settings["model"] = cleaned
    user_settings_path.write_text(
        json.dumps(settings, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )


def main() -> int:
    if len(sys.argv) not in {5, 6}:
        raise SystemExit(
            "usage: prepare-claude-config.py <user-claude-dir> <isolated-dir> "
            "<model> <effort> [max-context-tokens]"
        )
    user_claude = Path(sys.argv[1]).expanduser()
    isolated = Path(sys.argv[2]).expanduser()
    model = required_single_line(sys.argv[3], "model")
    effort = required_single_line(sys.argv[4], "effort")
    context_tokens = ""
    if len(sys.argv) == 6 and sys.argv[5]:
        context_tokens = required_single_line(sys.argv[5], "max-context-tokens")
        if not context_tokens.isdigit() or int(context_tokens) <= 0:
            raise ValueError("max-context-tokens must be a positive integer")

    user_settings = user_claude / "settings.json"
    # Keep plain `claude` free of claudex discovery model ids.
    sanitize_shared_settings(user_settings)
    mirror_shared_entries(user_claude, isolated)
    ensure_isolated_denylist(isolated)
    write_isolated_settings(
        user_settings, isolated / "settings.json", model, effort, context_tokens
    )
    # Print the absolute isolated path for the fish launcher.
    print(os.path.realpath(isolated))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:  # noqa: BLE001 - surface prepare failures to the launcher
        print(f"claudex: prepare-claude-config failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error
