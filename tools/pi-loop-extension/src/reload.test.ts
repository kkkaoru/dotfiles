// This TypeScript file is executed with Bun.
import { afterEach, expect, it, vi } from "vitest";
import { LoopRuntime, type LoopContext, type LoopHost } from "./runtime.ts";
import { createLoopState, latestLoopState, type LoopRuntimeState } from "./state.ts";

afterEach(() => {
  vi.useRealTimers();
});

function persistedPausedState(
  host: LoopHost,
  context: LoopContext,
  entries: readonly unknown[],
): LoopRuntimeState {
  const runtime = new LoopRuntime(host);
  runtime.command("inspect model", context);
  runtime.wakeup({ delaySeconds: 90, prompt: "inspect model", reason: "training" }, context);
  vi.advanceTimersByTime(30_000);
  runtime.command("pause", context);
  runtime.shutdown();
  const restored: LoopRuntimeState | undefined = latestLoopState(entries);
  if (restored === undefined) {
    throw new Error("Loop state was not persisted");
  }
  return restored;
}

it("delivers an in-flight tick after reload when idle", () => {
  vi.useFakeTimers();
  const sendUserMessage = vi.fn<LoopHost["sendUserMessage"]>();
  const runtime = new LoopRuntime({ sendUserMessage });
  const context: LoopContext = {
    isIdle: (): boolean => true,
    ui: { notify: vi.fn(), setStatus: vi.fn(), setWidget: vi.fn() },
  };
  runtime.restore(
    createLoopState({
      jobs: [],
      nextId: 2,
      paused: false,
      pendingContinuations: ["08-26 04:00 → 04:01 | loop=#110 | leftover\ncontinue"],
      runningContinuation: "08-26 04:00 → 04:01 | loop=#110 | leftover\ncontinue",
    }),
    context,
  );
  expect(sendUserMessage).not.toHaveBeenCalled();
  vi.runOnlyPendingTimers();
  expect(sendUserMessage).toHaveBeenCalledWith(
    "08-26 04:00 → 04:01 | loop=#110 | leftover\ncontinue",
    { deliverAs: "followUp" },
  );
  runtime.clear();
});

it("keeps paused leftovers visible and tells the user how to resume or clear", () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 8, 21, 23, 53));
  const sendUserMessage = vi.fn<LoopHost["sendUserMessage"]>();
  const notify = vi.fn<LoopContext["ui"]["notify"]>();
  const setWidget = vi.fn<NonNullable<LoopContext["ui"]["setWidget"]>>();
  const runtime = new LoopRuntime({ sendUserMessage });
  const context: LoopContext = {
    isIdle: (): boolean => true,
    ui: { notify, setStatus: vi.fn(), setWidget },
  };
  runtime.restore(
    createLoopState({
      jobs: [
        {
          id: 5,
          nextRunAt: new Date(2026, 8, 21, 4, 20).getTime(),
          prompt: "measure HUD",
          reason: "HUD parse ok",
          submittedAt: new Date(2026, 8, 21, 4, 19).getTime(),
        },
      ],
      nextId: 7,
      paused: true,
      pendingContinuations: ["09-21 04:19 → 04:20 | loop=#5 | HUD parse ok\nkeep measuring"],
      runningContinuation: undefined,
    }),
    context,
  );
  vi.runOnlyPendingTimers();
  runtime.agentSettled(context);
  expect(sendUserMessage).not.toHaveBeenCalled();
  expect(notify).toHaveBeenCalledWith(
    "Loop paused: 1 job(s), 1 ready. /loop resume or /loop clear.",
    "warning",
  );
  expect(setWidget).toHaveBeenLastCalledWith(
    "loop-wakeups",
    expect.arrayContaining(["09-21 04:19 → 04:20 | loop=#5 | HUD parse ok"]),
  );
  runtime.clear();
});

it("restores a paused wakeup across reload and resumes its remaining delay", () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 7, 26, 4, 0));
  const entries: unknown[] = [];
  const sendUserMessage = vi.fn<LoopHost["sendUserMessage"]>();
  const host: LoopHost = {
    appendEntry: (customType, data): void => {
      entries.push({ customType, data, type: "custom" });
    },
    sendUserMessage,
  };
  const context: LoopContext = {
    isIdle: (): boolean => true,
    ui: { notify: vi.fn(), setStatus: vi.fn(), setWidget: vi.fn() },
  };
  const restored: LoopRuntimeState = persistedPausedState(host, context, entries);
  sendUserMessage.mockClear();

  vi.setSystemTime(new Date(2026, 7, 26, 5, 0));
  const afterReload = new LoopRuntime(host);
  afterReload.restore(restored, context);
  afterReload.command("resume", context);
  const resumed: LoopRuntimeState | undefined = latestLoopState(entries);

  expect(resumed?.paused).toBe(false);
  expect(resumed?.jobs[0]?.nextRunAt).toBe(new Date(2026, 7, 26, 5, 1).getTime());
  expect(sendUserMessage).not.toHaveBeenCalled();
  afterReload.command("schedule next", context);
  expect(
    afterReload.wakeup({ delaySeconds: 60, prompt: "next", reason: "again" }, context).id,
  ).toBe(2);
  afterReload.clear();
});
