// This TypeScript file is executed with Bun.
import fs from "node:fs";
import { afterEach, expect, it, vi } from "vitest";
import {
  type Completion,
  type CompletionEvents,
  CompletionWaiter,
  createCompletionEvents,
  type StatusOperations,
  type WaitProcess,
} from "./waiter.ts";
import type { TmuxLaunch } from "./tmux.ts";

const launch: TmuxLaunch = {
  command: "tmux command",
  completionChannel: "pi-tmux-test-complete",
  logPath: "/tmp/pi-tmux-test/output.log",
  sessionName: "pi-tmux-test",
  socketName: "pi-tmux-socket",
  statusPath: "/tmp/pi-tmux-test/exit-status",
  submittedAt: "2026-06-01T01:02:03.000Z",
  taskCommand: "sleep 60",
};

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it("subscribes to tmux wait-for and handles its close event", () => {
  let close: ((code: number | null) => void) | undefined;
  const kill = vi.fn<WaitProcess["kill"]>();
  const process: WaitProcess = {
    kill,
    once: (_event, listener): void => {
      close = listener;
    },
  };
  const spawnProcess = vi.fn((): WaitProcess => process);
  const onSignal = vi.fn();
  const cancel = createCompletionEvents(spawnProcess).subscribe({
    channel: "pi-tmux-test-complete",
    onSignal,
    socketName: "pi-tmux-socket",
  });

  expect(spawnProcess).toHaveBeenCalledWith(
    "tmux",
    ["-L", "pi-tmux-socket", "wait-for", "pi-tmux-test-complete"],
    { stdio: "ignore" },
  );
  close?.(1);
  expect(onSignal).not.toHaveBeenCalled();
  close?.(0);
  expect(onSignal).toHaveBeenCalledOnce();
  cancel();
  expect(kill).toHaveBeenCalledWith("SIGTERM");
});

it("wakes exactly once from a valid tmux completion signal", () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-06-01T01:03:04.000Z"));
  let onSignal: (() => void) | undefined;
  const cancel = vi.fn();
  const events: CompletionEvents = {
    subscribe: ({ onSignal: signal }): (() => void) => {
      onSignal = signal;
      return cancel;
    },
  };
  const onComplete = vi.fn<(completion: Completion) => void>();
  const read = vi.fn<StatusOperations["read"]>().mockReturnValue("7\n");
  const waiter = new CompletionWaiter({ events, onComplete, operations: { read } });

  waiter.track(launch);
  waiter.track(launch);
  expect(cancel).toHaveBeenCalledOnce();
  onSignal?.();
  onSignal?.();
  expect(onComplete).toHaveBeenCalledOnce();
  expect(onComplete).toHaveBeenCalledWith({
    completedAt: "2026-06-01T01:03:04.000Z",
    exitCode: 7,
    launch,
  });
  waiter.clear();
});

it("ignores invalid and unreadable status files", () => {
  let onSignal: (() => void) | undefined;
  const events: CompletionEvents = {
    subscribe: ({ onSignal: signal }): (() => void) => {
      onSignal = signal;
      return (): void => undefined;
    },
  };
  const onComplete = vi.fn<(completion: Completion) => void>();
  const read = vi
    .fn<StatusOperations["read"]>()
    .mockReturnValueOnce("pending")
    .mockImplementationOnce((): string => {
      throw new Error("temporary read failure");
    });
  const waiter = new CompletionWaiter({ events, onComplete, operations: { read } });

  waiter.track(launch);
  onSignal?.();
  waiter.track(launch);
  onSignal?.();
  expect(onComplete).not.toHaveBeenCalled();
  waiter.cancel(launch);
  waiter.clear();
});

it("reconciles a completed status after a missed tmux signal", () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-06-01T01:03:05.000Z"));
  const cancel = vi.fn();
  const events: CompletionEvents = {
    subscribe: (): (() => void) => cancel,
  };
  const onComplete = vi.fn<(completion: Completion) => void>();
  const read = vi
    .fn<StatusOperations["read"]>()
    .mockReturnValueOnce("pending")
    .mockReturnValueOnce("0\n");
  const waiter = new CompletionWaiter({ events, onComplete, operations: { read } });

  waiter.track(launch);
  waiter.reconcile();
  expect(onComplete).not.toHaveBeenCalled();
  waiter.reconcile();
  expect(onComplete).toHaveBeenCalledWith({
    completedAt: "2026-06-01T01:03:05.000Z",
    exitCode: 0,
    launch,
  });
  expect(cancel).toHaveBeenCalledOnce();
  waiter.reconcile();
  expect(onComplete).toHaveBeenCalledOnce();
});

it("finishes an orphaned launch whose tmux session disappeared", () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-06-01T01:03:06.000Z"));
  const cancel = vi.fn();
  const events: CompletionEvents = {
    subscribe: (): (() => void) => cancel,
  };
  const onComplete = vi.fn<(completion: Completion) => void>();
  const waiter = new CompletionWaiter({
    events,
    onComplete,
    operations: {
      isRunning: (): boolean => false,
      read: (): string => {
        throw new Error("missing status");
      },
    },
  });

  waiter.track(launch);
  waiter.reconcile();

  expect(onComplete).toHaveBeenCalledWith({
    completedAt: "2026-06-01T01:03:06.000Z",
    exitCode: 255,
    launch,
    orphaned: true,
  });
  expect(cancel).toHaveBeenCalledOnce();
});

it("detects an orphan through the default tmux status operation", () => {
  const events: CompletionEvents = {
    subscribe: (): (() => void) => (): void => undefined,
  };
  const onComplete = vi.fn<(completion: Completion) => void>();
  const waiter = new CompletionWaiter({ events, onComplete });

  waiter.track({
    ...launch,
    sessionName: "pi-tmux-session-that-does-not-exist",
    statusPath: "/tmp/pi-tmux-status-that-does-not-exist",
  });
  waiter.reconcile();

  expect(onComplete).toHaveBeenCalledWith(
    expect.objectContaining({ exitCode: 255, orphaned: true }),
  );
});

it("uses mocked default status-file operations", () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-06-01T01:03:06.000Z"));
  let onSignal: (() => void) | undefined;
  vi.spyOn(fs, "readFileSync").mockReturnValue("0\n");
  const events: CompletionEvents = {
    subscribe: ({ onSignal: signal }): (() => void) => {
      onSignal = signal;
      return (): void => undefined;
    },
  };
  const onComplete = vi.fn<(completion: Completion) => void>();
  const waiter = new CompletionWaiter({ events, onComplete });

  waiter.track(launch);
  onSignal?.();
  expect(onComplete).toHaveBeenCalledWith({
    completedAt: "2026-06-01T01:03:06.000Z",
    exitCode: 0,
    launch,
  });
});
