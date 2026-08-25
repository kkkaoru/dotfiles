// This TypeScript file is executed with Bun.
import type { ToolCall } from "@earendil-works/pi-ai";
import { expect, it } from "vitest";
import { backgroundLongClaudexBash } from "./background-bash.ts";

it("backgrounds long Claude Code Bash calls with submission information", () => {
  expect(
    backgroundLongClaudexBash(
      {
        type: "toolCall",
        id: "bash-1",
        name: "Bash",
        arguments: { command: "bun run check", timeout: 120_000 },
      },
      new Date(2026, 7, 25, 23, 14, 59),
    ),
  ).toStrictEqual({
    type: "toolCall",
    id: "bash-1",
    name: "Bash",
    arguments: {
      command: "bun run check",
      timeout: 120_000,
      description: "08-25 23:14 | bun run check",
      run_in_background: true,
    },
  });
});

it("keeps a useful model-provided description and truncates it", () => {
  expect(
    backgroundLongClaudexBash(
      {
        type: "toolCall",
        id: "bash-2",
        name: "Bash",
        arguments: {
          command: "gh run watch 123",
          description: "Watch CI completion",
        },
      },
      new Date(2026, 7, 25, 23, 14, 59),
    ).arguments,
  ).toStrictEqual({
    command: "gh run watch 123",
    description: "08-25 23:14 | Watch CI completion",
    run_in_background: true,
  });
  expect(
    backgroundLongClaudexBash(
      {
        type: "toolCall",
        id: "bash-3",
        name: "Bash",
        arguments: { command: `watch ${"x".repeat(200)}` },
      },
      new Date(2026, 7, 25, 23, 14, 59),
    ).arguments["description"],
  ).toHaveLength(174);
});

it("leaves unrelated and malformed calls unchanged", () => {
  const short: ToolCall = {
    type: "toolCall",
    id: "bash-4",
    name: "Bash",
    arguments: { command: "bun test", timeout: 30_000 },
  };
  const explicit: ToolCall = {
    type: "toolCall",
    id: "bash-5",
    name: "Bash",
    arguments: { command: "watch date", run_in_background: true },
  };
  const malformed: ToolCall = {
    type: "toolCall",
    id: "bash-6",
    name: "Bash",
    arguments: { command: 42 },
  };
  const invalidTimeout: ToolCall = {
    type: "toolCall",
    id: "bash-7",
    name: "Bash",
    arguments: { command: "watch date", timeout: "slow" },
  };
  const invalidBackground: ToolCall = {
    type: "toolCall",
    id: "bash-8",
    name: "Bash",
    arguments: { command: "watch date", run_in_background: "yes" },
  };
  const read: ToolCall = {
    type: "toolCall",
    id: "read-1",
    name: "Read",
    arguments: { path: "README.md" },
  };

  expect(backgroundLongClaudexBash(short, new Date(2026, 7, 25, 23, 14, 59))).toBe(short);
  expect(backgroundLongClaudexBash(explicit, new Date(2026, 7, 25, 23, 14, 59))).toBe(explicit);
  expect(backgroundLongClaudexBash(malformed, new Date(2026, 7, 25, 23, 14, 59))).toBe(malformed);
  expect(backgroundLongClaudexBash(invalidTimeout, new Date(2026, 7, 25, 23, 14, 59))).toBe(
    invalidTimeout,
  );
  expect(backgroundLongClaudexBash(invalidBackground, new Date(2026, 7, 25, 23, 14, 59))).toBe(
    invalidBackground,
  );
  expect(backgroundLongClaudexBash(read, new Date(2026, 7, 25, 23, 14, 59))).toBe(read);
});
