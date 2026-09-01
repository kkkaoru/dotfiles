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
const setWidget = vi.fn<NonNullable<LoopContext["ui"]["setWidget"]>>();
const cleared = vi.fn();
let idle = true;
const RECURRING_NAME_PATTERN =
  /^\d{2}-\d{2} \d{2}:\d{2} → \d{2}:\d{2} \| loop=#1 \| Recurring every 5m\ncheck CI$/u;

const host: LoopHost = { sendUserMessage };
const context: LoopContext = {
  isIdle: (): boolean => idle,
  ui: { notify, setStatus, setWidget },
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
  vi.useRealTimers();
});

describe("LoopRuntime commands", () => {
  it("starts a self-paced prompted loop immediately", () => {
    const runtime = new LoopRuntime(host, scheduler);
    runtime.command("check the build", context);
    expect(sendUserMessage).toHaveBeenCalledOnce();
    expect(sendUserMessage.mock.calls[0]?.[0]).toBe(
      "This is a self-paced loop. Perform the task now and continue through every immediately actionable step. Do not end by merely reporting remaining work. Before ending, make exactly one terminal loop decision: call loop_wakeup when another useful later check remains, or call loop_complete only when the task is complete or blocked on user input. If neither tool is called, the loop automatically continues.\n\nTask:\ncheck the build",
    );
    expect(notify).toHaveBeenCalledWith("Started a self-paced loop.", "info");
  });

  it("uses conservative autonomous guidance for a bare loop", () => {
    const runtime = new LoopRuntime(host, scheduler);
    runtime.command("", context);

    expect(sendUserMessage.mock.calls[0]?.[0]).toBe(
      "This is a self-paced loop. Perform the task now and continue through every immediately actionable step. Do not end by merely reporting remaining work. Before ending, make exactly one terminal loop decision: call loop_wakeup when another useful later check remains, or call loop_complete only when the task is complete or blocked on user input. If neither tool is called, the loop automatically continues.\n\nTask:\nContinue work already established in this conversation. Act as a steward, not an initiator: finish in-progress work, verification, or clearly authorized maintenance. Do not invent new work or perform irreversible actions without authorization. If nothing actionable remains, say so briefly and stop.",
    );
  });

  it("polls, fires, rearms, lists, and clears a recurring loop", () => {
    const runtime = new LoopRuntime(host, scheduler);
    runtime.command("5m check CI", context);

    expect(callbacks[0]?.intervalMs).toBe(5000);
    expect(sendUserMessage).toHaveBeenCalledWith("check CI", { deliverAs: "followUp" });
    expect(notify).toHaveBeenCalledWith("Started loop #1 every 5m (session-scoped).", "info");
    expect(setStatus).toHaveBeenLastCalledWith("loop", "loop: 1");
    expect(setWidget).toHaveBeenLastCalledWith(
      "loop-wakeups",
      expect.arrayContaining([
        expect.stringMatching(/^\d{2}-\d{2} \d{2}:\d{2} → \d{2}:\d{2} \| loop=#1/u),
      ]),
    );
    currentTime = 61_000;
    runtime.command("list", context);
    expect(notify).toHaveBeenLastCalledWith("#1 in 4m: Recurring every 5m", "info");

    currentTime = 301_000;
    callbacks[0]?.callback();
    expect(sendUserMessage.mock.lastCall?.[0]).toMatch(RECURRING_NAME_PATTERN);
    expect(sendUserMessage.mock.lastCall?.[1]).toStrictEqual({ deliverAs: "followUp" });
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

describe("LoopRuntime delivery races", () => {
  it("requeues when sendUserMessage races with an active agent", () => {
    sendUserMessage.mockImplementationOnce((): never => {
      throw new Error("Agent is already processing a prompt. Use steer() or followUp().");
    });
    const runtime = new LoopRuntime(host, scheduler);

    expect((): void => runtime.command("check the build", context)).not.toThrow();
    expect(setWidget).toHaveBeenLastCalledWith("loop-wakeups", [
      expect.stringContaining("loop=self-paced"),
    ]);

    sendUserMessage.mockImplementationOnce((): never => {
      throw new Error("Agent is already processing a prompt. Use steer() or followUp().");
    });
    runtime.agentSettled(context);
    expect(setWidget).toHaveBeenLastCalledWith("loop-wakeups", [
      expect.stringContaining("loop=self-paced"),
    ]);
    runtime.agentSettled(context);

    expect(sendUserMessage).toHaveBeenCalledTimes(3);
    expect(setWidget).toHaveBeenLastCalledWith("loop-wakeups", undefined);
  });

  it("requeues an unfinished continuation when settled delivery races", () => {
    const runtime = new LoopRuntime(host, scheduler);
    runtime.command("check the build", context);
    sendUserMessage.mockImplementationOnce((): never => {
      throw new Error("Agent is already processing a prompt. Use steer() or followUp().");
    });

    runtime.agentSettled(context);
    runtime.agentSettled(context);

    expect(sendUserMessage).toHaveBeenCalledTimes(3);
    expect(notify).toHaveBeenCalledWith(expect.stringContaining("loop=self-paced"), "info");
  });

  it("does not hide unrelated delivery errors", () => {
    sendUserMessage.mockImplementationOnce((): never => {
      throw new Error("unexpected delivery failure");
    });
    const runtime = new LoopRuntime(host, scheduler);

    expect((): void => runtime.command("check the build", context)).toThrow(
      "unexpected delivery failure",
    );
  });
});

describe("LoopRuntime settled delivery", () => {
  it("defers and coalesces continuation until the lifecycle callback returns", () => {
    vi.useFakeTimers();
    const runtime = new LoopRuntime(host, scheduler);
    runtime.command("check deployment", context);

    runtime.deferLifecycleContinuation((): void => runtime.agentSettled(context));
    runtime.deferLifecycleContinuation((): void => runtime.agentSettled(context));
    expect(sendUserMessage).toHaveBeenCalledOnce();
    vi.runOnlyPendingTimers();

    expect(sendUserMessage).toHaveBeenCalledTimes(2);
  });

  it("cancels deferred continuation during shutdown", () => {
    vi.useFakeTimers();
    const runtime = new LoopRuntime(host, scheduler);
    runtime.command("check deployment", context);
    runtime.deferLifecycleContinuation((): void => runtime.agentSettled(context));

    runtime.shutdown();
    vi.runOnlyPendingTimers();

    expect(sendUserMessage).toHaveBeenCalledOnce();
  });
});

describe("LoopRuntime compaction", () => {
  it("continues an in-flight self-paced loop after non-retrying compaction", () => {
    const runtime = new LoopRuntime(host, scheduler);
    runtime.command("check deployment", context);
    idle = false;

    runtime.continueAfterCompaction(false, context);
    runtime.continueAfterCompaction(false, context);
    idle = true;
    runtime.agentSettled(context);

    expect(sendUserMessage).toHaveBeenCalledTimes(2);
    expect(sendUserMessage).toHaveBeenLastCalledWith(
      expect.stringMatching(
        /^\d{2}-\d{2} \d{2}:\d{2} → \d{2}:\d{2} \| loop=self-paced \| check deployment\nThis is a self-paced loop\./u,
      ),
      { deliverAs: "followUp" },
    );
    expect(notify).toHaveBeenCalledWith("Continuing loop after compaction.", "info");
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

  it("continues an unfinished tick until explicitly completed", () => {
    const runtime = new LoopRuntime(host, scheduler);
    runtime.command("check deployment", context);

    runtime.agentSettled(context);

    expect(sendUserMessage).toHaveBeenCalledTimes(2);
    expect(notify).toHaveBeenLastCalledWith("Continuing unfinished loop work.", "info");
    expect(runtime.complete("deployment verified", context)).toStrictEqual({
      reason: "deployment verified",
    });
    runtime.agentSettled(context);
    runtime.continueAfterCompaction(false, context);
    expect(sendUserMessage).toHaveBeenCalledTimes(2);
  });

  it("does not continue a cleared loop", () => {
    const runtime = new LoopRuntime(host, scheduler);
    runtime.command("check deployment", context);
    runtime.clear();
    runtime.agentSettled(context);
    runtime.continueAfterCompaction(false, context);

    expect(sendUserMessage).toHaveBeenCalledOnce();
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
    runtime.command("watch CI", context);
    expect(runtime.wakeup(input, context)).toStrictEqual({ id: 1, scheduledInSeconds: 90 });
    runtime.agentSettled(context);
    expect(sendUserMessage).toHaveBeenCalledOnce();

    currentTime = 500_000;
    idle = false;
    callbacks[0]?.callback();
    expect(sendUserMessage).toHaveBeenCalledOnce();
    expect(notify).toHaveBeenCalledWith(
      expect.stringMatching(/^\d{2}-\d{2} \d{2}:\d{2} → \d{2}:\d{2} \| loop=#1 \| CI may finish$/u),
      "info",
    );
    idle = true;
    runtime.agentSettled(context);
    expect(sendUserMessage).toHaveBeenCalledWith(
      expect.stringMatching(
        /^\d{2}-\d{2} \d{2}:\d{2} → \d{2}:\d{2} \| loop=#1 \| CI may finish\nThis is a self-paced loop\. Perform the task now and continue through every immediately actionable step\. Do not end by merely reporting remaining work\. Before ending, make exactly one terminal loop decision: call loop_wakeup when another useful later check remains, or call loop_complete only when the task is complete or blocked on user input\. If neither tool is called, the loop automatically continues\.\n\nTask:\ncheck again$/u,
      ),
      { deliverAs: "followUp" },
    );
    expect(cleared).toHaveBeenCalledOnce();
    runtime.command("list", context);
    expect(notify).toHaveBeenLastCalledWith("No loop jobs are scheduled.", "info");
  });

  it("validates wakeup inputs and requires an active tick", () => {
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
    expect(() =>
      runtime.wakeup({ delaySeconds: 60, prompt: "next", reason: "why" }, context),
    ).toThrow("loop_wakeup requires an active /loop tick");
  });

  it("validates completion and accepts only one terminal decision", () => {
    const runtime = new LoopRuntime(host, scheduler);
    expect(() => runtime.complete("done", context)).toThrow(
      "loop_complete requires an active /loop tick",
    );
    runtime.command("finish work", context);
    expect(() => runtime.complete(" ", context)).toThrow("reason must not be empty");
    expect(runtime.complete(" done ", context)).toStrictEqual({ reason: "done" });
    expect(() => runtime.complete("done again", context)).toThrow(
      "loop_complete requires an active /loop tick",
    );
  });
});
