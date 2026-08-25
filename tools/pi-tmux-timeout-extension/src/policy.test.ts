// This TypeScript file is executed with Bun.
import { expect, it } from "vitest";
import { formatLocalTimestamp, shouldBackgroundClaudexBash, shouldDetachBash } from "./policy.ts";

it("formats timestamps in the execution environment timezone", () => {
  const localDate = new Date(2026, 7, 25, 23, 14, 59);

  expect(formatLocalTimestamp(localDate, "submitted")).toBe("08-25 23:14");
  expect(formatLocalTimestamp(localDate, "completed")).toBe("23:14");
});

it("uses Pi timeout seconds for native Pi bash calls", () => {
  expect(shouldDetachBash({ command: "bun run check", timeout: 119 })).toBe(false);
  expect(shouldDetachBash({ command: "bun run check", timeout: 120 })).toBe(true);
});

it("uses Claude Code timeout milliseconds for Claudex bash calls", () => {
  expect(shouldBackgroundClaudexBash({ command: "bun run check", timeout: 119_999 })).toBe(false);
  expect(shouldBackgroundClaudexBash({ command: "bun run check", timeout: 120_000 })).toBe(true);
});

it("backgrounds Claudex watchers without overriding explicit background or tmux", () => {
  expect(shouldBackgroundClaudexBash({ command: "gh run watch 123" })).toBe(true);
  expect(shouldBackgroundClaudexBash({ command: "tail -f output.log" })).toBe(true);
  expect(shouldBackgroundClaudexBash({ command: "watch date" })).toBe(true);
  expect(shouldBackgroundClaudexBash({ command: "watch date", run_in_background: true })).toBe(
    false,
  );
  expect(
    shouldBackgroundClaudexBash({ command: "tmux new-session -d 'watch date'", timeout: 120_000 }),
  ).toBe(false);
});
