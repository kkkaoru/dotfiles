#!/usr/bin/env python3
"""PreToolUse (Bash): deny genuine `terraform apply` invocations.

Catches direct and wrapped forms (mise/aws-vault/env runners, -chdir, etc.).
Allows plan/validate/fmt/init and unrelated commands that merely mention the
string "terraform apply" inside an argument.

Deny rules like Bash(terraform apply:*) still help for unwrapped forms;
this hook covers wrappers and -chdir that permission globs miss or only
partially cover. Blocks even under bypassPermissions / --dangerously-skip-permissions.
"""
from __future__ import annotations

import json
import shlex
import sys

SHELL_OPS = frozenset({"&&", "||", ";", "|", "|&", "&", "\n"})
# Tokens that may precede a real command without ending the simple-command.
COMMAND_PREFIXES = frozenset(
    {
        "sudo",
        "command",
        "builtin",
        "time",
        "nice",
        "nohup",
        "stdbuf",
        "env",
        "xargs",
    }
)
# terraform global options that take a separate next argv (no '=' form).
TF_ARG_FLAGS = frozenset({"-chdir", "-var", "-var-file"})

DENY_REASON = (
    "terraform apply is always denied by harness policy. "
    "plan/validate/fmt/init remain allowed; apply only runs via CI after merge."
)


def _basename(token: str) -> str:
    return token.rsplit("/", 1)[-1]


def _is_terraform_bin(token: str) -> bool:
    # Real binary name only — not mise package pins like terraform@1.13.5.
    return _basename(token) == "terraform"


def _in_command_position(tokens: list[str], idx: int) -> bool:
    if idx == 0:
        return True
    prev = tokens[idx - 1]
    if prev in SHELL_OPS or prev == "--":
        return True
    if prev in COMMAND_PREFIXES:
        return True
    # Leading ENV=value assignments before a command.
    if idx >= 1 and "=" in prev and not prev.startswith("-") and not prev.startswith("="):
        # Walk back over consecutive assignments / prefixes.
        j = idx - 1
        while j >= 0:
            t = tokens[j]
            if t in SHELL_OPS:
                return True
            if t == "--" or t in COMMAND_PREFIXES:
                return True
            if "=" in t and not t.startswith("-"):
                j -= 1
                continue
            return False
        return True
    return False


def _terraform_subcommand(tokens: list[str], tf_idx: int) -> str | None:
    i = tf_idx + 1
    n = len(tokens)
    while i < n:
        t = tokens[i]
        if t in SHELL_OPS:
            return None
        if t.startswith("-"):
            flag = t.split("=", 1)[0]
            if "=" in t:
                i += 1
                continue
            if flag in TF_ARG_FLAGS:
                i += 2
                continue
            i += 1
            continue
        return t
    return None


def is_terraform_apply(command: str) -> bool:
    if not command or "terraform" not in command:
        return False
    try:
        tokens = shlex.split(command, posix=True)
    except ValueError:
        tokens = command.split()
    for i, tok in enumerate(tokens):
        if not _is_terraform_bin(tok):
            continue
        if not _in_command_position(tokens, i):
            continue
        if _terraform_subcommand(tokens, i) == "apply":
            return True
    return False


def main() -> int:
    raw = sys.stdin.read()
    try:
        payload = json.loads(raw) if raw.strip() else {}
    except json.JSONDecodeError:
        return 0
    command = (payload.get("tool_input") or {}).get("command") or ""
    if is_terraform_apply(command):
        json.dump(
            {
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": DENY_REASON,
                }
            },
            sys.stdout,
        )
        sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
