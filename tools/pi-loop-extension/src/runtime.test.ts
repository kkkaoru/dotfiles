// This TypeScript file is executed with Bun.
import { clearInterval, setInterval } from "node:timers";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  LoopRuntime,
  type LoopContext,
  type LoopHost,
  type Scheduler,
  type WakeupInput,
} from "./runtime.ts";

interface PollerCallback {
  readonly callback: () => void;
  readonly intervalMs: number;
  readonly poller: ReturnType<typeof setInterval>;
}

const pollers: ReturnType<typeof setInterval>[] = [];
let callbacks: PollerCallback[] = [];
let currentTime = 1000;
const sendUserMessage = vi.fn<LoopHost["sendUserMessage"]>();
const notify = vi.fn<LoopContext["ui"]["notify"]>();
const setStatus = vi.fn<LoopContext["ui"]["setStatus"]>();
const cleared = vi.fn();
let idle = true;

const host: LoopHost = { sendUserMessage };
const context: LoopContext = {
  isIdle: (): boolean => idle,
  ui: { notify, setStatus },
};
const scheduler: Scheduler = {
  clearInterval: (poller): void => {
    cleared();
    clearInterval(poller);
  },
  now: (): number => currentTime,
  setInterval: (callback, intervalMs) => {
    const poller = setInterval((): void => undefined, 1_000_000);
    pollers.push(poller);
    callbacks.push({ callback, intervalMs, poller });
    return poller;
  },
};

beforeEach(() => {
  callbacks = [];
  currentTime = 1000;
  idle = true;
});

afterEach(() => {
  pollers.map((poller): void => clearInterval(poller));
  pollers.length = 0;
});

describe("LoopRuntime commands", () => {
  it("starts a self-paced prompted loop immediately", () => {
    const runtime = new LoopRuntime(host, scheduler);
    runtime.command("check the build", context);

    expect(sendUserMessage).toHaveBeenCalledOnce();
    expect(sendUserMessage.mock.calls[0]?.[0]).toBe(
      "This is a self-paced loop. Perform the task now. Keep the session responsive: when a command is expected to run for a long time and tmux is available, start it in a detached tmux session with output and exit status redirected to files instead of waiting in the foreground. Preserve the tmux session and file paths in the next wakeup prompt, then inspect them on a later tick. Run short commands normally. Before ending, call loop_wakeup only when another useful check remains. Do not schedule another wakeup when the task is complete, blocked on user input, or waiting on external state that cannot be checked later.\n\nTask:\ncheck the build",
    );
    expect(notify).toHaveBeenCalledWith("Started a self-paced loop.", "info");
  });

  it("uses conservative autonomous guidance for a bare loop", () => {
    const runtime = new LoopRuntime(host, scheduler);
    runtime.command("", context);

    expect(sendUserMessage.mock.calls[0]?.[0]).toBe(
      "This is a self-paced loop. Perform the task now. Keep the session responsive: when a command is expected to run for a long time and tmux is available, start it in a detached tmux session with output and exit status redirected to files instead of waiting in the foreground. Preserve the tmux session and file paths in the next wakeup prompt, then inspect them on a later tick. Run short commands normally. Before ending, call loop_wakeup only when another useful check remains. Do not schedule another wakeup when the task is complete, blocked on user input, or waiting on external state that cannot be checked later.\n\nTask:\nContinue work already established in this conversation. Act as a steward, not an initiator: finish in-progress work, verification, or clearly authorized maintenance. Do not invent new work or perform irreversible actions without authorization. If nothing actionable remains, say so briefly and stop.",
    );
  });

  it("polls, fires, rearms, lists, and clears a recurring loop", () => {
    const runtime = new LoopRuntime(host, scheduler);
    runtime.command("5m check CI", context);

    expect(callbacks[0]?.intervalMs).toBe(5000);
    expect(sendUserMessage).toHaveBeenCalledWith("check CI", {});
    expect(notify).toHaveBeenCalledWith("Started loop #1 every 5m (session-scoped).", "info");
    expect(setStatus).toHaveBeenLastCalledWith("loop", "loop: 1");

    currentTime = 61_000;
    runtime.command("list", context);
    expect(notify).toHaveBeenLastCalledWith("#1 in 4m: Recurring every 5m", "info");

    currentTime = 301_000;
    callbacks[0]?.callback();
    expect(sendUserMessage).toHaveBeenLastCalledWith("check CI", {});
    runtime.command("list", context);
    expect(notify).toHaveBeenLastCalledWith("#1 in 5m: Recurring every 5m", "info");

    runtime.command("clear", context);
    expect(notify).toHaveBeenLastCalledWith("Cleared 1 loop job(s).", "info");
    expect(setStatus).toHaveBeenLastCalledWith("loop", undefined);
    expect(cleared).toHaveBeenCalledOnce();
  });

  it("pauses and resumes jobs while preserving their remaining delays", () => {
    const runtime = new LoopRuntime(host, scheduler);
    runtime.command("5m check CI", context);
    currentTime = 61_000;
    runtime.command("pause", context);

    expect(notify).toHaveBeenLastCalledWith("Paused 1 loop job(s).", "info");
    expect(setStatus).toHaveBeenLastCalledWith("loop", "loop: 1 (paused)");
    runtime.command("list", context);
    expect(notify).toHaveBeenLastCalledWith("#1 paused, in 4m: Recurring every 5m", "info");

    currentTime = 601_000;
    callbacks[0]?.callback();
    expect(sendUserMessage).toHaveBeenCalledTimes(1);
    runtime.command("pause", context);
    expect(notify).toHaveBeenLastCalledWith("Loop jobs are already paused.", "info");

    runtime.command("resume", context);
    expect(notify).toHaveBeenLastCalledWith("Resumed 1 loop job(s).", "info");
    runtime.command("resume", context);
    expect(notify).toHaveBeenLastCalledWith("Loop jobs are not paused.", "info");

    currentTime = 841_000;
    callbacks[1]?.callback();
    runtime.clear();
  });

  it("reports an empty job list and supports pausing before a new job", () => {
    const runtime = new LoopRuntime(host, scheduler);
    runtime.command("list", context);
    expect(notify).toHaveBeenCalledWith("No loop jobs are scheduled.", "info");
    runtime.command("pause", context);
    runtime.command("1m later", context);
    expect(callbacks).toStrictEqual([]);
    runtime.command("resume", context);
    expect(callbacks[0]?.intervalMs).toBe(5000);
    expect(runtime.clear()).toBe(1);
    expect(runtime.clear()).toBe(0);
  });
});

describe("LoopRuntime compaction", () => {
  it("continues an in-flight self-paced loop after non-retrying compaction", () => {
    const runtime = new LoopRuntime(host, scheduler);
    runtime.command("check deployment", context);
    idle = false;

    runtime.continueAfterCompaction(false, context);

    expect(sendUserMessage).toHaveBeenCalledTimes(2);
    expect(sendUserMessage).toHaveBeenLastCalledWith(
      "This is a self-paced loop. Perform the task now. Keep the session responsive: when a command is expected to run for a long time and tmux is available, start it in a detached tmux session with output and exit status redirected to files instead of waiting in the foreground. Preserve the tmux session and file paths in the next wakeup prompt, then inspect them on a later tick. Run short commands normally. Before ending, call loop_wakeup only when another useful check remains. Do not schedule another wakeup when the task is complete, blocked on user input, or waiting on external state that cannot be checked later.\n\nTask:\ncheck deployment",
      { deliverAs: "followUp" },
    );
    expect(notify).toHaveBeenLastCalledWith("Continuing loop after compaction.", "info");
  });

  it("does not duplicate continuation when pi retries or a recurring job remains", () => {
    const retryingRuntime = new LoopRuntime(host, scheduler);
    retryingRuntime.command("check deployment", context);
    retryingRuntime.continueAfterCompaction(true, context);
    expect(sendUserMessage).toHaveBeenCalledOnce();

    const recurringRuntime = new LoopRuntime(host, scheduler);
    recurringRuntime.command("5m check deployment", context);
    recurringRuntime.continueAfterCompaction(false, context);
    expect(sendUserMessage).toHaveBeenCalledTimes(2);
    recurringRuntime.clear();
  });

  it("does not continue a loop that already settled or was cleared", () => {
    const settledRuntime = new LoopRuntime(host, scheduler);
    settledRuntime.command("check deployment", context);
    settledRuntime.agentSettled();
    settledRuntime.continueAfterCompaction(false, context);
    expect(sendUserMessage).toHaveBeenCalledOnce();

    const clearedRuntime = new LoopRuntime(host, scheduler);
    clearedRuntime.command("check deployment", context);
    clearedRuntime.clear();
    clearedRuntime.continueAfterCompaction(false, context);
    expect(sendUserMessage).toHaveBeenCalledTimes(2);
  });
});

describe("LoopRuntime tmux detachment", () => {
  it("detaches a long-timeout bash command while a loop tick is running", () => {
    const runtime = new LoopRuntime(host, scheduler);
    const input = { command: "gh run watch 32847265628 --exit-status --compact", timeout: 1200 };
    runtime.command("watch CI", context);

    expect(runtime.detachLongRunningBash(input)).toBe(true);
    expect(input.timeout).toBe(30);
    expect(input.command).toMatch(/tmux new-session -d -s 'pi-loop-/u);
    expect(input.command).toMatch(/output\.log/u);
    expect(input.command).toMatch(/exit-status/u);
    expect(input.command).toMatch(/Schedule a loop_wakeup/u);
  });

  it("detects known watch commands without a long timeout", () => {
    const runtime = new LoopRuntime(host, scheduler);
    const input: { command: string; timeout?: number } = { command: "tail -f server.log" };
    runtime.command("watch logs", context);

    expect(runtime.detachLongRunningBash(input)).toBe(true);
    expect(input.timeout).toBe(30);
  });

  it("leaves short, already-detached, and non-loop commands unchanged", () => {
    const inactiveRuntime = new LoopRuntime(host, scheduler);
    expect(
      inactiveRuntime.detachLongRunningBash({ command: "gh run watch 1", timeout: 1200 }),
    ).toBe(false);

    const activeRuntime = new LoopRuntime(host, scheduler);
    activeRuntime.command("check", context);
    expect(activeRuntime.detachLongRunningBash({ command: "bun test", timeout: 30 })).toBe(false);
    expect(
      activeRuntime.detachLongRunningBash({
        command: "tmux new-session -d -s existing 'gh run watch 1'",
        timeout: 1200,
      }),
    ).toBe(false);
  });
});

describe("LoopRuntime wakeups", () => {
  it("fires an overdue one-shot after polling resumes from sleep", () => {
    const runtime = new LoopRuntime(host, scheduler);
    const input: WakeupInput = {
      delaySeconds: 90,
      prompt: " check again ",
      reason: " CI may finish ",
    };
    expect(runtime.wakeup(input, context)).toStrictEqual({ id: 1, scheduledInSeconds: 90 });

    currentTime = 500_000;
    idle = false;
    callbacks[0]?.callback();
    expect(sendUserMessage).toHaveBeenCalledWith("check again", { deliverAs: "followUp" });
    expect(cleared).toHaveBeenCalledOnce();
    runtime.command("list", context);
    expect(notify).toHaveBeenLastCalledWith("No loop jobs are scheduled.", "info");
  });

  it("validates wakeup inputs", () => {
    const runtime = new LoopRuntime(host, scheduler);
    expect(() =>
      runtime.wakeup({ delaySeconds: 60.5, prompt: "next", reason: "why" }, context),
    ).toThrow("delaySeconds must be an integer");
    expect(() =>
      runtime.wakeup({ delaySeconds: 59, prompt: "next", reason: "why" }, context),
    ).toThrow("delaySeconds must be between 60 and 3,600");
    expect(() =>
      runtime.wakeup({ delaySeconds: 3601, prompt: "next", reason: "why" }, context),
    ).toThrow("delaySeconds must be between 60 and 3,600");
    expect(() => runtime.wakeup({ delaySeconds: 60, prompt: " ", reason: "why" }, context)).toThrow(
      "prompt and reason must not be empty",
    );
    expect(() =>
      runtime.wakeup({ delaySeconds: 60, prompt: "next", reason: " " }, context),
    ).toThrow("prompt and reason must not be empty");
  });
});
