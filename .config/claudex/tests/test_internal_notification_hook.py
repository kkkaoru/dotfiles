#!/usr/bin/env python3
"""Regression tests for the claudex UserPromptSubmit notification boundary."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import time
import unittest


ROOT = Path(__file__).resolve().parents[3]
ROUTING_SCRIPTS = ROOT / ".claude/skills/claudex-routing/scripts"
sys.path.insert(0, str(ROUTING_SCRIPTS))
import route_usage  # noqa: E402


class InternalNotificationHookTests(unittest.TestCase):
    def run_hook(self, prompt: str, **environment: str) -> subprocess.CompletedProcess[str]:
        env = {**os.environ, **environment}
        return subprocess.run(
            ["python3", str(route_usage.__file__)],
            input=json.dumps({"user_prompt": prompt}),
            capture_output=True,
            text=True,
            check=False,
            env=env,
        )

    def test_blocks_pure_agent_notification_for_claudex_parent(self) -> None:
        result = self.run_hook(
            '<agent-message from="worker">completed</agent-message>',
            CLAUDEX_ACTIVE="1",
        )
        self.assertEqual(result.returncode, 0)
        self.assertEqual(
            json.loads(result.stdout),
            {
                "decision": "block",
                "reason": "Claudex internal background notification consumed",
            },
        )

    def test_blocks_wrapped_task_notification(self) -> None:
        result = self.run_hook(
            "Another Claude session sent a message: "
            "<task-notification><status>completed</status></task-notification>",
            CLAUDEX_ACTIVE="1",
        )
        self.assertEqual(result.returncode, 0)
        self.assertEqual(json.loads(result.stdout)["decision"], "block")

    def test_preserves_real_prompts_and_teammate_prompts(self) -> None:
        for prompt in (
            "Please explain the literal <agent-message> tag.",
            "<teammate-message>Investigate this task.</teammate-message>",
        ):
            with self.subTest(prompt=prompt):
                result = self.run_hook(prompt, CLAUDEX_ACTIVE="1")
                self.assertEqual(result.returncode, 0)
                self.assertNotIn('"decision":"block"', result.stdout)

    def test_is_inert_outside_parent_and_when_opted_in(self) -> None:
        prompt = '<agent-message from="worker">completed</agent-message>'
        for environment in ({}, {"CLAUDEX_ACTIVE": "1", "CLAUDEX_AGMSG_AUTO_MONITOR": "1"}):
            with self.subTest(environment=environment):
                result = self.run_hook(prompt, **environment)
                self.assertEqual(result.returncode, 0)
                self.assertNotIn('"decision":"block"', result.stdout)

    def test_classifier_does_not_match_literal_or_teammate_text(self) -> None:
        self.assertTrue(
            route_usage.is_internal_notification_prompt(
                '<agent-message from="worker">completed</agent-message>'
            )
        )
        self.assertTrue(
            route_usage.is_internal_notification_prompt(
                "Another Claude session sent a message: "
                "<task-notification><status>completed</status></task-notification>"
            )
        )
        self.assertFalse(
            route_usage.is_internal_notification_prompt(
                "Please explain the literal <agent-message> tag."
            )
        )
        self.assertFalse(
            route_usage.is_internal_notification_prompt(
                "<teammate-message>Investigate this task.</teammate-message>"
            )
        )

    def test_delayed_hook_payload_is_still_blocked(self) -> None:
        process = subprocess.Popen(
            ["python3", str(route_usage.__file__)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env={**os.environ, "CLAUDEX_ACTIVE": "1"},
        )
        assert process.stdin is not None
        time.sleep(0.1)
        process.stdin.write(
            json.dumps(
                {
                    "user_prompt": "<agent-message from=\"delayed\">done</agent-message>"
                }
            )
        )
        process.stdin.flush()
        process.stdin.close()
        assert process.stdout is not None
        assert process.stderr is not None
        stdout = process.stdout.read()
        stderr = process.stderr.read()
        process.stdout.close()
        process.stderr.close()
        process.wait(timeout=2)
        self.assertEqual(process.returncode, 0, stderr)
        self.assertEqual(json.loads(stdout)["decision"], "block")


if __name__ == "__main__":
    unittest.main()
