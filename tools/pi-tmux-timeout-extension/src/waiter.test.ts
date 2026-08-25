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
  statusPath: "/tmp/pi-tmux-test/exit-status",
  submittedAt: "2026-06-01T01:02:03.000Z",
  taskCommand: "sleep 60",
};

afterEach(() => {
  vi.restoreAllMocks();
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
  });

  expect(spawnProcess).toHaveBeenCalledWith("tmux", ["wait-for", "pi-tmux-test-complete"], {
    stdio: "ignore",
  });
  close?.(1);
  expect(onSignal).not.toHaveBeenCalled();
  close?.(0);
  expect(onSignal).toHaveBeenCalledOnce();
  cancel();
  expect(kill).toHaveBeenCalledWith("SIGTERM");
});

it("wakes exactly once from a valid tmux completion signal", () => {
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
  expect(onComplete).toHaveBeenCalledWith({ exitCode: 7, launch });
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
  expect(onComplete).toHaveBeenCalledWith({ exitCode: 0, launch });
  expect(cancel).toHaveBeenCalledOnce();
  waiter.reconcile();
  expect(onComplete).toHaveBeenCalledOnce();
});

it("uses mocked default status-file operations", () => {
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
  expect(onComplete).toHaveBeenCalledWith({ exitCode: 0, launch });
});
