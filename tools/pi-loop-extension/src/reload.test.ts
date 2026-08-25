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

it("delivers a ready continuation when reload restores into an idle session", () => {
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
      pendingContinuations: ["08-26 04:00 → 04:01 | loop=#1 | inspect\ncontinue"],
      runningContinuation: undefined,
    }),
    context,
  );

  expect(sendUserMessage).toHaveBeenCalledWith("08-26 04:00 → 04:01 | loop=#1 | inspect\ncontinue");
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

  vi.setSystemTime(new Date(2026, 7, 26, 5, 0));
  const afterReload = new LoopRuntime(host);
  afterReload.restore(restored, context);
  afterReload.command("resume", context);
  const resumed: LoopRuntimeState | undefined = latestLoopState(entries);

  expect(resumed?.paused).toBe(false);
  expect(resumed?.jobs[0]?.nextRunAt).toBe(new Date(2026, 7, 26, 5, 1).getTime());
  expect(sendUserMessage).not.toHaveBeenCalled();
  expect(
    afterReload.wakeup({ delaySeconds: 60, prompt: "next", reason: "again" }, context).id,
  ).toBe(2);
  afterReload.clear();
});
