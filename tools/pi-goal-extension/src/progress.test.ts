// Runs with Bun; all persistence, delivery and clocks are mocked.
import { EventEmitter } from "node:events";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ActivityProvider, announceTask } from "./activity.ts";
import { type GoalHost, GoalRuntime } from "./runtime.ts";

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(1000);
});
afterEach(() => {
  vi.clearAllTimers();
  vi.useRealTimers();
});
function setup() {
  const emitter: EventEmitter = new EventEmitter();
  const host = {
    sessionId: "session",
    isReady: () => true,
    send: vi.fn<(text: string) => void>(),
    persist: vi.fn(),
    display: vi.fn(),
    bus: {
      emit: (channel: string, data: unknown) => {
        emitter.emit(channel, data);
      },
      on: (channel: string, listener: (data: unknown) => void) => {
        emitter.on(channel, listener);
        return () => {
          emitter.off(channel, listener);
        };
      },
    },
  } satisfies GoalHost;
  const runtime: GoalRuntime = new GoalRuntime(host);
  runtime.restore([]);
  runtime.start({ objective: "Verify change", tokenBudget: null });
  return { runtime, host };
}
it("stops repeated text-only turns; a goal read alone is not work evidence", () => {
  const { runtime } = setup();
  runtime.begin();
  runtime.recordTool("get_goal");
  runtime.end(null);
  runtime.begin();
  runtime.end(null);
  runtime.begin();
  runtime.end(null);
  expect(runtime.state?.status).toBe("blocked");
  expect(runtime.state?.noProgressTurns).toBe(3);
  runtime.resume();
  expect(runtime.state?.noProgressTurns).toBe(0);
});
it("successful tool activity, explicit waits and blocker audits reset stalled counts", () => {
  const { runtime } = setup();
  runtime.begin();
  runtime.end(null);
  runtime.begin();
  runtime.recordTool("read");
  runtime.end(null);
  expect(runtime.state?.noProgressTurns).toBe(0);
  runtime.begin();
  runtime.wait({ delaySeconds: 60, reason: "Check external status" });
  runtime.end(null);
  expect(runtime.state?.noProgressTurns).toBe(0);
  runtime.begin();
  runtime.update("blocked", "Missing authorization");
  runtime.end(null);
  expect(runtime.state?.noProgressTurns).toBe(0);
});
it("waits for owned live tasks and prevents completion while their notification is pending", () => {
  const { runtime, host } = setup();
  const snapshot: { tasks: string[]; pending: boolean } = {
    tasks: ["unrelated"],
    pending: false,
  };
  const provider: ActivityProvider = new ActivityProvider(host.bus, () => ({
    source: "tmux",
    ownsContinuation: false,
    pendingDelivery: snapshot.pending,
    tasks: snapshot.tasks,
  }));
  provider.start("session");
  runtime.begin();
  runtime.end(null);
  expect(runtime.state?.noProgressTurns).toBe(1);
  announceTask(host.bus, { sessionId: "session", name: "owned" });
  snapshot.tasks = ["owned"];
  runtime.begin();
  runtime.end(null);
  expect(runtime.state?.noProgressTurns).toBe(0);
  snapshot.tasks = [];
  snapshot.pending = true;
  runtime.begin();
  runtime.end(null);
  expect(runtime.state?.noProgressTurns).toBe(0);
  expect(() => runtime.update("complete", "Not yet inspected")).toThrow(
    "Inspect owned tmux tasks",
  );
  snapshot.pending = false;
  runtime.update("complete", "Inspected exit status and artifacts");
  expect(runtime.state?.status).toBe("complete");
  provider.stop();
});
it("changing a wait invalidates already queued active-goal tickets", () => {
  const { runtime, host } = setup();
  vi.advanceTimersByTime(5000);
  const text: string | undefined = host.send.mock.calls[0]?.[0];
  if (text === undefined) throw new Error("Expected queued continuation");
  runtime.wait({ delaySeconds: 60, reason: "External check" });
  expect(runtime.accept(text)).toBe(false);
  vi.advanceTimersByTime(60000);
  expect(host.send).toHaveBeenCalledTimes(2);
});
it("does not attribute a foreign foreground run or its error to a newly created goal", () => {
  const { runtime } = setup();
  runtime.clear();
  runtime.begin();
  runtime.start({ objective: "New goal", tokenBudget: null });
  runtime.recordTokens(100);
  runtime.end("Old unrelated run failed.");
  runtime.settled();
  expect(runtime.state?.tokensUsed).toBe(0);
  expect(runtime.state?.status).toBe("active");
  runtime.recordTokens(2);
  expect(runtime.state?.tokensUsed).toBe(2);
  runtime.begin();
  runtime.clear();
  runtime.start({ objective: "Replacement", tokenBudget: null });
  runtime.recordTokens(100);
  runtime.end("Old goal failed.");
  runtime.recordTokens(3);
  expect(runtime.state?.tokensUsed).toBe(3);
  expect(runtime.state?.status).toBe("active");
});
