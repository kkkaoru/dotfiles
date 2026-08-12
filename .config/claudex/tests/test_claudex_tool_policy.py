#!/usr/bin/env python3
"""Tests for claudex main-delegation and file-lock PreToolUse policy (Rust binary)."""

from __future__ import annotations

import json
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest


ROOT = Path(__file__).resolve().parents[3]


def resolve_policy_bin() -> Path:
    candidates = [
        ROOT / "tools/claudex-tool-policy/target/debug/claudex-tool-policy",
        ROOT / "tools/claudex-tool-policy/target/release/claudex-tool-policy",
        Path("/tmp/claudex-tool-policy-target/debug/claudex-tool-policy"),
        Path.home() / ".cargo/bin/claudex-tool-policy",
    ]
    which = shutil.which("claudex-tool-policy")
    if which:
        candidates.append(Path(which))
    for path in candidates:
        if path.is_file() and os.access(path, os.X_OK):
            return path
    raise FileNotFoundError(
        "claudex-tool-policy binary not found; run "
        "`cargo install --path tools/claudex-tool-policy`"
    )


POLICY = resolve_policy_bin()


class ClaudexToolPolicyTests(unittest.TestCase):
    @staticmethod
    def write_state(
        cache: Path,
        session_id: str,
        *,
        base_required: bool = True,
        prompt_opt_out: bool = False,
        selected_workers_count: int = 2,
        updated_at: float | None = None,
        expires_at: float | None = None,
    ) -> Path:
        key = hashlib.sha256(session_id.encode()).hexdigest()
        if updated_at is None:
            updated_at = time.time()
        directory = cache / "delegation-state-v2"
        directory.mkdir()
        directory.chmod(0o700)
        required = base_required and not prompt_opt_out
        state = {
            "version": 2,
            "session_key": key,
            "updated_at": updated_at,
            "expires_at": updated_at + 86_400.0 if expires_at is None else expires_at,
            "base_delegation_required": base_required,
            "prompt_opt_out": prompt_opt_out,
            "delegation_required": required,
            "selected_workers_count": selected_workers_count,
            "direct_main_execution": "fallback-only" if required else "allowed",
        }
        path = directory / f"{key}.json"
        path.write_text(json.dumps(state), encoding="utf-8")
        path.chmod(0o600)
        return path

    def run_policy(
        self,
        payload: dict,
        *,
        cache: Path,
        env: dict[str, str] | None = None,
    ) -> dict:
        process_env = {
            **os.environ,
            "CLAUDEX_ACTIVE": "1",
            "CLAUDEX_SUBAGENT_FIRST": "1",
            "CLAUDEX_CACHE_DIR": str(cache),
            **(env or {}),
        }
        result = subprocess.run(
            [str(POLICY)],
            input=json.dumps(payload),
            capture_output=True,
            text=True,
            check=False,
            env=process_env,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout or "{}")

    def test_main_session_bash_allowed_when_delegation_required(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cache = Path(tmp)
            self.write_state(cache, "sess-main")
            output = self.run_policy(
                {
                    "hook_event_name": "PreToolUse",
                    "tool_name": "Bash",
                    "tool_input": {"command": "ls"},
                    "session_id": "sess-main",
                },
                cache=cache,
            )
            self.assertEqual(output, {})

    def test_main_session_read_denied_when_delegation_required(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cache = Path(tmp)
            self.write_state(cache, "sess-main")
            output = self.run_policy(
                {
                    "hook_event_name": "PreToolUse",
                    "tool_name": "Write",
                    "tool_input": {"file_path": "/tmp/x", "content": "x"},
                    "session_id": "sess-main",
                },
                cache=cache,
            )
            decision = output["hookSpecificOutput"]["permissionDecision"]
            self.assertEqual(decision, "deny")
            self.assertIn("Agent/Task", output["hookSpecificOutput"]["permissionDecisionReason"])

    def test_subagent_bash_allowed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cache = Path(tmp)
            self.write_state(cache, "sess-main", selected_workers_count=1)
            output = self.run_policy(
                {
                    "hook_event_name": "PreToolUse",
                    "tool_name": "Bash",
                    "tool_input": {"command": "ls"},
                    "session_id": "sess-main",
                    "agent_id": "agent-1",
                    "agent_type": "claudex-gpt",
                },
                cache=cache,
            )
            self.assertEqual(output, {})

    def test_subagent_read_explicitly_allowed_despite_main_denylist(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cache = Path(tmp)
            self.write_state(cache, "sess-main")
            output = self.run_policy(
                {
                    "hook_event_name": "PreToolUse",
                    "tool_name": "Read",
                    "tool_input": {"file_path": "/tmp/x"},
                    "session_id": "sess",
                    "agent_id": "agent-worker",
                    "agent_type": "claudex-fugu",
                },
                cache=cache,
            )
            self.assertEqual(
                output["hookSpecificOutput"]["permissionDecision"],
                "allow",
            )
            self.assertIn(
                "do not apply",
                output["hookSpecificOutput"]["permissionDecisionReason"].lower(),
            )

    def test_subagent_detected_via_transcript_path_without_agent_id(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cache = Path(tmp)
            self.write_state(cache, "sess-main")
            output = self.run_policy(
                {
                    "hook_event_name": "PreToolUse",
                    "tool_name": "Grep",
                    "tool_input": {"pattern": "x"},
                    "session_id": "sess",
                    "transcript_path": "/tmp/projects/abc/subagents/agent-xyz.jsonl",
                },
                cache=cache,
            )
            self.assertEqual(
                output["hookSpecificOutput"]["permissionDecision"],
                "allow",
            )

    def test_file_lock_blocks_second_writer(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cache = Path(tmp)
            target = Path(tmp) / "shared.rs"
            target.write_text("fn main() {}\n", encoding="utf-8")
            first = self.run_policy(
                {
                    "hook_event_name": "PreToolUse",
                    "tool_name": "Edit",
                    "tool_input": {"file_path": str(target), "old_string": "a", "new_string": "b"},
                    "session_id": "sess",
                    "agent_id": "agent-a",
                },
                cache=cache,
            )
            self.assertNotEqual(
                first.get("hookSpecificOutput", {}).get("permissionDecision"),
                "deny",
            )
            second = self.run_policy(
                {
                    "hook_event_name": "PreToolUse",
                    "tool_name": "Write",
                    "tool_input": {"file_path": str(target), "content": "x"},
                    "session_id": "sess",
                    "agent_id": "agent-b",
                },
                cache=cache,
            )
            self.assertEqual(
                second["hookSpecificOutput"]["permissionDecision"],
                "deny",
            )
            self.assertIn("agent-a", second["hookSpecificOutput"]["permissionDecisionReason"])

    def test_subagent_stop_releases_locks(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cache = Path(tmp)
            target = Path(tmp) / "owned.rs"
            target.write_text("ok\n", encoding="utf-8")
            self.run_policy(
                {
                    "hook_event_name": "PreToolUse",
                    "tool_name": "Write",
                    "tool_input": {"file_path": str(target), "content": "ok"},
                    "session_id": "sess",
                    "agent_id": "agent-a",
                },
                cache=cache,
            )
            self.run_policy(
                {
                    "hook_event_name": "SubagentStop",
                    "agent_id": "agent-a",
                    "session_id": "sess",
                },
                cache=cache,
            )
            allowed = self.run_policy(
                {
                    "hook_event_name": "PreToolUse",
                    "tool_name": "Write",
                    "tool_input": {"file_path": str(target), "content": "next"},
                    "session_id": "sess",
                    "agent_id": "agent-b",
                },
                cache=cache,
            )
            self.assertNotEqual(
                allowed.get("hookSpecificOutput", {}).get("permissionDecision"),
                "deny",
            )

    def test_allow_main_tools_override(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cache = Path(tmp)
            self.write_state(cache, "sess")
            output = self.run_policy(
                {
                    "hook_event_name": "PreToolUse",
                    "tool_name": "Write",
                    "tool_input": {"file_path": "/tmp/x"},
                    "session_id": "sess",
                },
                cache=cache,
                env={"CLAUDEX_ALLOW_MAIN_TOOLS": "1"},
            )
            self.assertEqual(output, {})

    def test_short_ttl_state_is_ignored(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cache = Path(tmp)
            now = time.time()
            self.write_state(
                cache, "sess", updated_at=now, expires_at=now + 86_399.0
            )
            output = self.run_policy(
                {
                    "hook_event_name": "PreToolUse",
                    "tool_name": "Write",
                    "tool_input": {"file_path": "/tmp/x", "content": "x"},
                    "session_id": "sess",
                },
                cache=cache,
            )
            self.assertEqual(output, {})

    def test_cache_override_does_not_split_from_home_cache(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp) / "home"
            home.mkdir()
            override = Path(tmp) / "override"
            override.mkdir()
            self.write_state(override, "sess")
            output = self.run_policy(
                {
                    "hook_event_name": "PreToolUse",
                    "tool_name": "Write",
                    "tool_input": {"file_path": "/tmp/x", "content": "x"},
                    "session_id": "sess",
                },
                cache=override,
                env={"HOME": str(home)},
            )
            self.assertEqual(
                output["hookSpecificOutput"]["permissionDecision"], "deny"
            )


if __name__ == "__main__":
    unittest.main()
