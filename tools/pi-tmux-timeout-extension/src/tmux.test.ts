// This TypeScript file is executed with Bun.
import { expect, it } from "vitest";
import {
  createTmuxLaunch,
  shouldDetachBash,
  TMUX_LAUNCH_TIMEOUT_SECONDS,
  tmuxSessionNamespace,
  TmuxRuntime,
} from "./tmux.ts";

const SESSION_ID = "01a03e61-5a77-7345-bb03-adda2f5fd100";
const SESSION_NAMESPACE = tmuxSessionNamespace(SESSION_ID);

it("creates a quoted detached tmux launch command", () => {
  const launch = createTmuxLaunch("sleep 1; echo 'tmux ok'", 7, SESSION_NAMESPACE);

  expect(launch.sessionName).toBe(`pi-tmux-${SESSION_NAMESPACE}-7`);
  expect(launch.socketName).toBe(`pi-tmux-${SESSION_NAMESPACE}`);
  expect(launch.completionChannel).toBe(`pi-tmux-${SESSION_NAMESPACE}-7-complete`);
  expect(launch.logPath).toMatch(new RegExp(`pi-tmux-${SESSION_NAMESPACE}-7/output\\.log$`, "u"));
  expect(launch.statusPath).toMatch(new RegExp(`pi-tmux-${SESSION_NAMESPACE}-7/exit-status$`, "u"));
  expect(launch.submittedAt).toMatch(/^\d{4}-\d{2}-\d{2}T/u);
  expect(launch.taskCommand).toBe("sleep 1; echo 'tmux ok'");
  expect(launch.command).toContain(
    `tmux -L 'pi-tmux-${SESSION_NAMESPACE}' new-session -d -s 'pi-tmux-${SESSION_NAMESPACE}-7'`,
  );
  expect(launch.command).toMatch(/echo '"'"'tmux ok'"'"'/u);
  expect(launch.command).toMatch(/tmux -L .* wait-for -S/u);
  expect(launch.command).toContain("launch.json");
  expect(launch.command).toContain("sleep 1; echo");
  expect((): ReturnType<TmuxRuntime["createLaunch"]> =>
    createTmuxLaunch("sleep 1", 1, "invalid"),
  ).toThrow("invalid tmux session namespace");
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
  const tracked: string[] = [];
  const runtime = new TmuxRuntime({
    onComplete: (): void => undefined,
    onTrack: (launch): void => {
      tracked.push(launch.sessionName);
    },
  });
  runtime.startSession(SESSION_ID);
  const shortInput = { command: "bun test", timeout: 30 };
  const longInput = { command: "gh run watch 123", timeout: 1200 };

  expect(runtime.rewriteLongBash(shortInput)).toBeUndefined();
  expect(runtime.rewriteLongBash(longInput)?.sessionName).toBe(`pi-tmux-${SESSION_NAMESPACE}-1`);
  expect(longInput.timeout).toBe(TMUX_LAUNCH_TIMEOUT_SECONDS);
  expect(longInput.command).toContain(`pi-tmux-${SESSION_NAMESPACE}-1`);
  expect(runtime.createLaunch("sleep 1").sessionName).toBe(`pi-tmux-${SESSION_NAMESPACE}-2`);
  expect(tracked).toHaveLength(0);
  const trackedLaunch = runtime.createLaunch("sleep 2");
  runtime.trackLaunch(trackedLaunch);
  expect(tracked).toEqual([trackedLaunch.sessionName]);
  runtime.clear();
});

it("restores launch tracking and advances ids beyond recovered jobs", () => {
  const subscribed: string[] = [];
  const runtime = new TmuxRuntime({
    events: {
      subscribe: ({ channel }): (() => void) => {
        subscribed.push(channel);
        return (): void => undefined;
      },
    },
    onComplete: (): void => undefined,
    operations: {
      read: (): string => {
        throw new Error("still running");
      },
    },
  });
  runtime.startSession(SESSION_ID);
  const recovered = createTmuxLaunch("sleep 10", 11, SESSION_NAMESPACE);

  const malformed = { ...recovered, completionChannel: "invalid-complete", sessionName: "invalid" };
  const otherNamespace = tmuxSessionNamespace("another-main-session");
  const otherSession = createTmuxLaunch("sleep 30", 99, otherNamespace);
  runtime.restore([recovered, malformed, otherSession], 20);

  expect(subscribed).toEqual([recovered.completionChannel]);
  const fresh = runtime.createLaunch("sleep 20");
  expect(fresh.sessionName).toBe(`pi-tmux-${SESSION_NAMESPACE}-20`);
  runtime.trackLaunch(fresh);
  runtime.restore([]);
  runtime.clear();
});

it("uses distinct tmux servers and counters for different Pi sessions", () => {
  const runtime = new TmuxRuntime({ onComplete: (): void => undefined });
  runtime.startSession("main-session-a");
  const first = runtime.createLaunch("sleep 1");
  runtime.startSession("main-session-b");
  const second = runtime.createLaunch("sleep 1");

  expect(first.sessionName).not.toBe(second.sessionName);
  expect(first.socketName).not.toBe(second.socketName);
  expect(first.sessionName).toMatch(/-1$/u);
  expect(second.sessionName).toMatch(/-1$/u);
});
