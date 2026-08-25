// This TypeScript file is executed with Bun.
import { expect, it } from "vitest";
import {
  createTmuxLaunch,
  shouldDetachBash,
  TMUX_LAUNCH_TIMEOUT_SECONDS,
  TmuxRuntime,
} from "./tmux.ts";

it("creates a quoted detached tmux launch command", () => {
  const launch = createTmuxLaunch("sleep 1; echo 'tmux ok'", 7);

  expect(launch.sessionName).toMatch(/^pi-tmux-\d+-7$/u);
  expect(launch.completionChannel).toMatch(/^pi-tmux-\d+-7-complete$/u);
  expect(launch.logPath).toMatch(/pi-tmux-\d+-7\/output\.log$/u);
  expect(launch.statusPath).toMatch(/pi-tmux-\d+-7\/exit-status$/u);
  expect(launch.submittedAt).toMatch(/^\d{4}-\d{2}-\d{2}T/u);
  expect(launch.taskCommand).toBe("sleep 1; echo 'tmux ok'");
  expect(launch.command).toMatch(/tmux new-session -d -s 'pi-tmux-\d+-7'/u);
  expect(launch.command).toMatch(/echo '"'"'tmux ok'"'"'/u);
  expect(launch.command).toMatch(/tmux wait-for -S/u);
});

it("detects long timeouts and known watch commands", () => {
  expect(shouldDetachBash({ command: "bun run check", timeout: 120 })).toBe(true);
  expect(shouldDetachBash({ command: "gh run watch 123" })).toBe(true);
  expect(shouldDetachBash({ command: "tail -f server.log" })).toBe(true);
  expect(shouldDetachBash({ command: "watch date" })).toBe(true);
});

it("leaves short and already detached commands unchanged", () => {
  expect(shouldDetachBash({ command: "bun test", timeout: 30 })).toBe(false);
  expect(
    shouldDetachBash({
      command: "tmux new-session -d -s existing 'gh run watch 1'",
      timeout: 1200,
    }),
  ).toBe(false);
});

it("rewrites matching bash calls and preserves the counter for skipped calls", () => {
  const runtime = new TmuxRuntime({ onComplete: (): void => undefined });
  const shortInput = { command: "bun test", timeout: 30 };
  const longInput = { command: "gh run watch 123", timeout: 1200 };

  expect(runtime.rewriteLongBash(shortInput)).toBeUndefined();
  expect(runtime.rewriteLongBash(longInput)?.sessionName).toMatch(/^pi-tmux-\d+-1$/u);
  expect(longInput.timeout).toBe(TMUX_LAUNCH_TIMEOUT_SECONDS);
  expect(longInput.command).toMatch(/pi-tmux-\d+-1/u);
  expect(runtime.createLaunch("sleep 1").sessionName).toMatch(/pi-tmux-\d+-2/u);
  runtime.clear();
});
