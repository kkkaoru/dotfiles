import { clearTimeout, setTimeout } from "node:timers";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  LoopRuntime,
  type LoopContext,
  type LoopHost,
  type Scheduler,
  type WakeupInput,
} from "./runtime.ts";

interface ScheduledCallback {
  readonly callback: () => void;
  readonly delayMs: number;
  readonly timer: ReturnType<typeof setTimeout>;
}

const timers: ReturnType<typeof setTimeout>[] = [];
let callbacks: ScheduledCallback[] = [];
let currentTime = 1000;
const sendUserMessage = vi.fn<LoopHost["sendUserMessage"]>();
const notify = vi.fn<LoopContext["ui"]["notify"]>();
const setStatus = vi.fn<LoopContext["ui"]["setStatus"]>();
let idle = true;

const host: LoopHost = { sendUserMessage };
const context: LoopContext = {
  isIdle: (): boolean => idle,
  ui: { notify, setStatus },
};
const scheduler: Scheduler = {
  clearTimeout: (timer): void => clearTimeout(timer),
  now: (): number => currentTime,
  setTimeout: (callback, delayMs) => {
    const timer = setTimeout((): void => undefined, 1_000_000);
    timers.push(timer);
    callbacks.push({ callback, delayMs, timer });
    return timer;
  },
};

beforeEach(() => {
  callbacks = [];
  currentTime = 1000;
  idle = true;
});

afterEach(() => {
  timers.map((timer): void => clearTimeout(timer));
  timers.length = 0;
});

describe("LoopRuntime commands", () => {
  it("starts a self-paced prompted loop immediately", () => {
    const runtime = new LoopRuntime(host, scheduler);
    runtime.command("check the build", context);

    expect(sendUserMessage).toHaveBeenCalledOnce();
    expect(sendUserMessage.mock.calls[0]?.[0]).toBe(
      "This is a self-paced loop. Perform the task now. Before ending, call loop_wakeup only when another useful check remains. Do not schedule another wakeup when the task is complete, blocked on user input, or waiting on external state that cannot be checked later.\n\nTask:\ncheck the build",
    );
    expect(notify).toHaveBeenCalledWith("Started a self-paced loop.", "info");
  });

  it("uses conservative autonomous guidance for a bare loop", () => {
    const runtime = new LoopRuntime(host, scheduler);
    runtime.command("", context);

    expect(sendUserMessage.mock.calls[0]?.[0]).toBe(
      "This is a self-paced loop. Perform the task now. Before ending, call loop_wakeup only when another useful check remains. Do not schedule another wakeup when the task is complete, blocked on user input, or waiting on external state that cannot be checked later.\n\nTask:\nContinue work already established in this conversation. Act as a steward, not an initiator: finish in-progress work, verification, or clearly authorized maintenance. Do not invent new work or perform irreversible actions without authorization. If nothing actionable remains, say so briefly and stop.",
    );
  });

  it("starts, fires, rearms, lists, and clears a recurring loop", () => {
    const runtime = new LoopRuntime(host, scheduler);
    runtime.command("5m check CI", context);

    expect(callbacks[0]?.delayMs).toBe(300_000);
    expect(sendUserMessage).toHaveBeenCalledWith("check CI", {});
    expect(notify).toHaveBeenCalledWith("Started loop #1 every 5m (session-scoped).", "info");
    expect(setStatus).toHaveBeenLastCalledWith("loop", "loop: 1");

    currentTime = 61_000;
    runtime.command("list", context);
    expect(notify).toHaveBeenLastCalledWith("#1 in 4m: Recurring every 5m", "info");

    callbacks[0]?.callback();
    expect(sendUserMessage).toHaveBeenLastCalledWith("check CI", {});
    expect(callbacks[1]?.delayMs).toBe(300_000);

    runtime.command("clear", context);
    expect(notify).toHaveBeenLastCalledWith("Cleared 1 loop job(s).", "info");
    expect(setStatus).toHaveBeenLastCalledWith("loop", undefined);
    callbacks[1]?.callback();
    expect(sendUserMessage).toHaveBeenCalledTimes(2);
  });

  it("reports an empty job list", () => {
    const runtime = new LoopRuntime(host, scheduler);
    runtime.command("list", context);
    expect(notify).toHaveBeenCalledWith("No loop jobs are scheduled.", "info");
    expect(runtime.clear()).toBe(0);
  });
});

describe("LoopRuntime wakeups", () => {
  it("schedules a one-shot wakeup and queues it while busy", () => {
    const runtime = new LoopRuntime(host, scheduler);
    const input: WakeupInput = {
      delaySeconds: 90,
      prompt: " check again ",
      reason: " CI may finish ",
    };
    expect(runtime.wakeup(input, context)).toStrictEqual({ id: 1, scheduledInSeconds: 90 });

    idle = false;
    callbacks[0]?.callback();
    expect(sendUserMessage).toHaveBeenCalledWith("check again", { deliverAs: "followUp" });
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
