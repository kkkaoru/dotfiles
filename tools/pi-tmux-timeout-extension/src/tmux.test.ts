// This TypeScript file is executed with Bun.
import { afterEach, expect, it, vi } from "vitest";
import {
  createTmuxLaunch,
  shouldDetachBash,
  TMUX_LAUNCH_TIMEOUT_SECONDS,
  tmuxSessionNamespace,
  TmuxRuntime,
} from "./tmux.ts";
import type { Completion } from "./waiter.ts";

const SESSION_ID = "01a03e61-5a77-7345-bb03-adda2f5fd100";
const SESSION_NAMESPACE = tmuxSessionNamespace(SESSION_ID);

afterEach(() => {
  vi.useRealTimers();
});

it("creates a quoted detached tmux launch command", () => {
  const launch = createTmuxLaunch({
    command: "sleep 1; echo 'tmux ok'",
    estimatedDurationSeconds: 360,
    id: 7,
    namespace: SESSION_NAMESPACE,
  });

  expect(launch.sessionName).toBe(`pi-tmux-${SESSION_NAMESPACE}-7`);
  expect(launch.socketName).toBe(`pi-tmux-${SESSION_NAMESPACE}`);
  expect(launch.completionChannel).toBe(`pi-tmux-${SESSION_NAMESPACE}-7-complete`);
  expect(launch.logPath).toMatch(new RegExp(`pi-tmux-${SESSION_NAMESPACE}-7/output\\.log$`, "u"));
  expect(launch.statusPath).toMatch(new RegExp(`pi-tmux-${SESSION_NAMESPACE}-7/exit-status$`, "u"));
  expect(launch.submittedAt).toMatch(/^\d{4}-\d{2}-\d{2}T/u);
  expect(Date.parse(launch.estimatedCompletionAt ?? "") - Date.parse(launch.submittedAt)).toBe(
    360_000,
  );
  expect(launch.taskCommand).toBe("sleep 1; echo 'tmux ok'");
  expect(launch.command).toContain(
    `tmux -L 'pi-tmux-${SESSION_NAMESPACE}' new-session -d -s 'pi-tmux-${SESSION_NAMESPACE}-7'`,
  );
  expect(launch.command).toMatch(/echo '"'"'tmux ok'"'"'/u);
  expect(launch.command).toMatch(/tmux -L .* wait-for -S/u);
  expect(launch.command).toContain("launch.json");
  expect(launch.command).toContain("sleep 1; echo");
  expect((): ReturnType<TmuxRuntime["createLaunch"]> =>
    createTmuxLaunch({ command: "sleep 1", id: 1, namespace: "invalid" }),
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
  const rewrittenLaunch = runtime.rewriteLongBash(longInput);
  expect(rewrittenLaunch?.sessionName).toBe(`pi-tmux-${SESSION_NAMESPACE}-1`);
  expect(longInput.timeout).toBe(TMUX_LAUNCH_TIMEOUT_SECONDS);
  expect(longInput.command).toContain(`pi-tmux-${SESSION_NAMESPACE}-1`);
  expect(
    Date.parse(rewrittenLaunch?.estimatedCompletionAt ?? "") -
      Date.parse(rewrittenLaunch?.submittedAt ?? ""),
  ).toBe(1_200_000);
  expect(runtime.createLaunch("sleep 1").sessionName).toBe(`pi-tmux-${SESSION_NAMESPACE}-2`);
  expect(tracked).toHaveLength(0);
  const trackedLaunch = runtime.createLaunch("sleep 2");
  runtime.trackLaunch(trackedLaunch);
  expect(tracked).toEqual([trackedLaunch.sessionName]);
  runtime.clear();
});

it("publishes active launches and removes them on completion", () => {
  const activeSnapshots: string[][] = [];
  const completionSignals: (() => void)[] = [];
  const completions: string[] = [];
  const runtime = new TmuxRuntime({
    events: {
      subscribe: ({ onSignal }): (() => void) => {
        completionSignals.push(onSignal);
        return (): void => undefined;
      },
    },
    onActiveChange: (launches): void => {
      activeSnapshots.push(launches.map((launch) => launch.sessionName));
    },
    onComplete: (completion): void => {
      completions.push(completion.launch.sessionName);
    },
    operations: { read: (): string => "0\n" },
  });
  runtime.startSession(SESSION_ID);
  const launch = runtime.createLaunch("sleep 10");

  runtime.trackLaunch(launch);
  completionSignals[0]?.();

  expect(activeSnapshots).toStrictEqual([[], ["pi-tmux-20bf694ba86ff02acbb2056489968728-1"], []]);
  expect(completions).toStrictEqual(["pi-tmux-20bf694ba86ff02acbb2056489968728-1"]);
});

it("periodically removes an orphaned active launch", () => {
  vi.useFakeTimers();
  const activeSnapshots: string[][] = [];
  const completions: Completion[] = [];
  const runtime = new TmuxRuntime({
    events: { subscribe: (): (() => void) => (): void => undefined },
    onActiveChange: (launches): void => {
      activeSnapshots.push(launches.map((launch) => launch.sessionName));
    },
    onComplete: (completion): void => {
      completions.push(completion);
    },
    operations: {
      isRunning: (): boolean => false,
      read: (): string => {
        throw new Error("missing status");
      },
    },
  });
  runtime.startSession(SESSION_ID);
  runtime.trackLaunch(runtime.createLaunch("orphaned job"));

  vi.advanceTimersByTime(60_000);

  expect(activeSnapshots).toStrictEqual([[], ["pi-tmux-20bf694ba86ff02acbb2056489968728-1"], []]);
  expect(completions).toStrictEqual([expect.objectContaining({ exitCode: 255, orphaned: true })]);
  runtime.clear();
});

it("restores launch tracking and advances ids beyond recovered jobs", () => {
  const activeSnapshots: string[][] = [];
  const subscribed: string[] = [];
  const runtime = new TmuxRuntime({
    events: {
      subscribe: ({ channel }): (() => void) => {
        subscribed.push(channel);
        return (): void => undefined;
      },
    },
    onActiveChange: (launches): void => {
      activeSnapshots.push(launches.map((launch) => launch.sessionName));
    },
    onComplete: (): void => undefined,
    operations: {
      read: (): string => {
        throw new Error("still running");
      },
    },
  });
  runtime.startSession(SESSION_ID);
  const recovered = createTmuxLaunch({ command: "sleep 10", id: 11, namespace: SESSION_NAMESPACE });

  const malformed = { ...recovered, completionChannel: "invalid-complete", sessionName: "invalid" };
  const otherNamespace = tmuxSessionNamespace("another-main-session");
  const otherSession = createTmuxLaunch({
    command: "sleep 30",
    id: 99,
    namespace: otherNamespace,
  });
  runtime.restore([recovered, malformed, otherSession], 20);

  expect(subscribed).toStrictEqual(["pi-tmux-20bf694ba86ff02acbb2056489968728-11-complete"]);
  expect(activeSnapshots).toStrictEqual([[], ["pi-tmux-20bf694ba86ff02acbb2056489968728-11"]]);
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
