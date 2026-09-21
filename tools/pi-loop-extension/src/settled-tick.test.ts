// This TypeScript file is executed with Bun.
import { expect, it, vi } from "vitest";
import type { LoopHost } from "./contracts.ts";
import {
  ABANDONED_LOOP_NOTICE,
  CONTINUED_LOOP_NOTICE,
  settleTick,
  type SettledTickActions,
} from "./settled-tick.ts";

function actions(): SettledTickActions {
  return {
    abandon: vi.fn(),
    clearPending: vi.fn(),
    clearRunning: vi.fn(),
    markContinued: vi.fn(),
    notify: vi.fn(),
    persist: vi.fn(),
    queue: vi.fn(),
    updateStatus: vi.fn(),
  };
}

function host(): LoopHost {
  return {
    sendUserMessage: (content: string): void => {
      if (content.length === 0) {
        throw new Error("empty continuation");
      }
    },
  };
}

it("delivers pending work, continues once, then abandons", () => {
  const pendingActions = actions();
  const pendingHost = host();
  settleTick(
    { continuedWithoutTerminal: false, jobs: 0, pending: ["ready"], running: "tick" },
    pendingActions,
    pendingHost,
  );
  expect(pendingActions.clearPending).toHaveBeenCalledOnce();
  expect(pendingActions.markContinued).not.toHaveBeenCalled();

  const continueActions = actions();
  settleTick(
    { continuedWithoutTerminal: false, jobs: 0, pending: [], running: "tick" },
    continueActions,
    host(),
  );
  expect(continueActions.notify).toHaveBeenCalledWith(CONTINUED_LOOP_NOTICE, "info");
  expect(continueActions.markContinued).toHaveBeenCalledOnce();

  const stopActions = actions();
  settleTick(
    { continuedWithoutTerminal: true, jobs: 0, pending: [], running: "tick" },
    stopActions,
    host(),
  );
  expect(stopActions.abandon).toHaveBeenCalledOnce();
  expect(ABANDONED_LOOP_NOTICE).toContain("loop_complete");
});

it("clears a running tick when jobs remain and queues a busy continuation", () => {
  const withJobs = actions();
  settleTick(
    { continuedWithoutTerminal: false, jobs: 1, pending: [], running: "tick" },
    withJobs,
    host(),
  );
  expect(withJobs.clearRunning).toHaveBeenCalledOnce();

  const busy = actions();
  const busyHost: LoopHost = {
    sendUserMessage: (): void => {
      throw new Error("Agent is already processing a prompt");
    },
  };
  settleTick(
    { continuedWithoutTerminal: false, jobs: 0, pending: [], running: "tick" },
    busy,
    busyHost,
  );
  expect(busy.queue).toHaveBeenCalledWith("tick");
});
