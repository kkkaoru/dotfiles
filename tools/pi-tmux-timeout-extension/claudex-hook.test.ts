// This TypeScript file is executed with Bun.
import { expect, it } from "vitest";
import { claudexHookOutput } from "./claudex-hook.ts";

it("updates matching Claude Code Bash input without approving it", () => {
  expect(
    claudexHookOutput(
      {
        hook_event_name: "PreToolUse",
        tool_name: "Bash",
        tool_input: {
          command: "sleep 20",
          description: "Validate background completion",
          timeout: 120_000,
        },
      },
      new Date(2026, 7, 25, 23, 14, 59),
    ),
  ).toStrictEqual({
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      updatedInput: {
        command: "sleep 20",
        description: "08-25 23:14 | Validate background completion",
        timeout: 120_000,
        run_in_background: true,
      },
    },
  });
});

it("ignores unrelated, malformed, explicit-background, and tmux calls", () => {
  expect(claudexHookOutput(null, new Date(2026, 7, 25, 23, 14, 59))).toBeUndefined();
  expect(
    claudexHookOutput(
      { hook_event_name: "PostToolUse", tool_name: "Bash", tool_input: {} },
      new Date(2026, 7, 25, 23, 14, 59),
    ),
  ).toBeUndefined();
  expect(
    claudexHookOutput(
      { hook_event_name: "PreToolUse", tool_name: "Bash", tool_input: null },
      new Date(2026, 7, 25, 23, 14, 59),
    ),
  ).toBeUndefined();
  expect(
    claudexHookOutput(
      { hook_event_name: "PreToolUse", tool_name: "Bash", tool_input: { command: 42 } },
      new Date(2026, 7, 25, 23, 14, 59),
    ),
  ).toBeUndefined();
  expect(
    claudexHookOutput(
      {
        hook_event_name: "PreToolUse",
        tool_name: "Bash",
        tool_input: { command: "watch date", run_in_background: true },
      },
      new Date(2026, 7, 25, 23, 14, 59),
    ),
  ).toBeUndefined();
  expect(
    claudexHookOutput(
      {
        hook_event_name: "PreToolUse",
        tool_name: "Bash",
        tool_input: { command: "tmux new-session -d 'watch date'", timeout: 120_000 },
      },
      new Date(2026, 7, 25, 23, 14, 59),
    ),
  ).toBeUndefined();
});
