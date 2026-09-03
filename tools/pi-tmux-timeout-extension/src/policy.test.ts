// This TypeScript file is executed with Bun.
import { expect, it } from "vitest";
import { formatLocalTimestamp, shouldBackgroundClaudexBash, shouldDetachBash } from "./policy.ts";

it("formats timestamps in the execution environment timezone", () => {
  const localDate = new Date(2026, 7, 25, 23, 14, 59);

  expect(formatLocalTimestamp(localDate, "submitted")).toBe("08-25 23:14");
  expect(formatLocalTimestamp(localDate, "completed")).toBe("23:14");
});

it("uses a thirty-second foreground budget for Pi bash calls", () => {
  expect(shouldDetachBash({ command: "echo bounded", timeout: 29 })).toBe(false);
  expect(shouldDetachBash({ command: "echo uncertain", timeout: 30 })).toBe(true);
});

it("detaches potentially blocking Pi commands without a foreground budget", () => {
  expect(shouldDetachBash({ command: "bun run check" })).toBe(true);
  expect(shouldDetachBash({ command: "uv run pytest" })).toBe(true);
  expect(shouldDetachBash({ command: "python custom-workflow.py" })).toBe(true);
  expect(shouldDetachBash({ command: "docker run --rm app" })).toBe(true);
  expect(shouldDetachBash({ command: "wrangler deploy" })).toBe(true);
  expect(shouldDetachBash({ command: "alembic upgrade head" })).toBe(true);
  expect(shouldDetachBash({ command: "curl https://example.com/status" })).toBe(true);
  expect(shouldDetachBash({ command: "git diff --stat -- app" })).toBe(true);
  expect(shouldDetachBash({ command: "command -v actionlint && actionlint workflow.yml" })).toBe(
    true,
  );
});

it("keeps bounded or confidently short local Pi commands in foreground", () => {
  expect(shouldDetachBash({ command: "curl https://example.com/status", timeout: 10 })).toBe(false);
  expect(shouldDetachBash({ command: "git status --short" })).toBe(false);
  expect(shouldDetachBash({ command: "echo ok && git status --short" })).toBe(false);
  expect(shouldDetachBash({ command: "rg -n TODO src", timeout: 20 })).toBe(false);
  expect(shouldDetachBash({ command: "tmux new-session -d 'bun run check'", timeout: 300 })).toBe(
    false,
  );
});

it("uses a thirty-second foreground budget for Claudex bash calls", () => {
  expect(shouldBackgroundClaudexBash({ command: "echo bounded", timeout: 29_999 })).toBe(false);
  expect(shouldBackgroundClaudexBash({ command: "echo uncertain", timeout: 30_000 })).toBe(true);
});

it("backgrounds uncertain Claudex work without overriding explicit choices", () => {
  expect(shouldBackgroundClaudexBash({ command: "bun run check" })).toBe(true);
  expect(shouldBackgroundClaudexBash({ command: "gh run watch 123" })).toBe(true);
  expect(
    shouldBackgroundClaudexBash({ command: "curl https://example.com", timeout: 10_000 }),
  ).toBe(false);
  expect(shouldBackgroundClaudexBash({ command: "watch date", run_in_background: true })).toBe(
    false,
  );
  expect(
    shouldBackgroundClaudexBash({
      command: "tmux new-session -d 'watch date'",
      timeout: 120_000,
    }),
  ).toBe(false);
});
