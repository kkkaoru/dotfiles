// Runs with Bun; clocks, delivery and session I/O are mocked.
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
    isReady: vi.fn(() => true),
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
  return { host, runtime };
}

it("adopts an agent-defined goal into the current run and accounts subsequent work", () => {
  const { runtime } = setup();
  runtime.begin();
  runtime.startFromAgent("Verify the user's requested build");
  runtime.recordTokens(20);
  runtime.recordTool("read");
  runtime.end(null);
  expect(runtime.state).toMatchObject({
    status: "active",
    turn: 1,
    tokensUsed: 20,
    tokenBudget: null,
    noProgressTurns: 0,
  });
  runtime.update("complete", "Build and tests verified.");
  expect(runtime.state?.status).toBe("complete");
});

it("refuses agent replacement or resumption of an unfinished or paused goal", () => {
  const { runtime } = setup();
  runtime.startFromAgent("authorized work");
  expect(() => runtime.startFromAgent("replacement")).toThrow(
    "unfinished goal",
  );
  runtime.pause();
  expect(() => runtime.startFromAgent("bypass pause")).toThrow(
    "unfinished goal",
  );
  expect(runtime.state?.status).toBe("paused");
});

it("records a verified completion for a stopped goal without resuming it", () => {
  const { host, runtime } = setup();
  runtime.start({ objective: "task", tokenBudget: null });
  runtime.begin();
  runtime.end(
    "Provider error; inspect the original Pi error and explicitly resume.",
  );
  runtime.settled();
  expect(runtime.state?.status).toBe("paused");
  expect(() => runtime.update("blocked", "still stuck")).toThrow("not active");
  runtime.update("complete", "Verified against current artifacts");
  runtime.end(null);
  runtime.settled();
  expect(runtime.state?.status).toBe("complete");
  vi.advanceTimersByTime(5000);
  expect(host.send).not.toHaveBeenCalled();
  expect(() => runtime.update("complete", "again")).toThrow("already complete");
});
it("continues after successful context recovery without requiring manual resume", () => {
  const { host, runtime } = setup();
  runtime.start({ objective: "recover", tokenBudget: null });
  runtime.begin();
  runtime.end(
    "Provider error; inspect the original Pi error and explicitly resume.",
  );
  runtime.compactionFinished(true);
  runtime.settled();
  vi.advanceTimersByTime(5000);
  expect(runtime.state?.status).toBe("active");
  expect(host.send).toHaveBeenCalledOnce();
});

it("enters safe mode after failed recovery and does not retry indefinitely", () => {
  const { host, runtime } = setup();
  runtime.start({ objective: "recover", tokenBudget: null });
  runtime.compactionFinished(false);
  runtime.settled();
  runtime.compactionFinished(true);
  vi.advanceTimersByTime(60_000);
  expect(runtime.state?.status).toBe("paused");
  expect(runtime.state?.reason).toMatch(/safe mode/u);
  expect(host.send).not.toHaveBeenCalled();
});

it("never resumes manual pauses or creates a goal after successful compaction", () => {
  const { host, runtime } = setup();
  runtime.compactionFinished(true);
  expect(runtime.state).toBeNull();
  runtime.start({ objective: "recover", tokenBudget: null });
  runtime.pause();
  runtime.compactionFinished(true);
  runtime.settled();
  vi.advanceTimersByTime(60_000);
  expect(runtime.state?.status).toBe("paused");
  expect(host.send).not.toHaveBeenCalled();
});

it("does not mistake a user abort for a recovered provider error", () => {
  const { runtime } = setup();
  runtime.start({ objective: "recover", tokenBudget: null });
  runtime.begin();
  runtime.end("Run aborted; only the user can resume the goal.");
  runtime.compactionFinished(true);
  runtime.settled();
  expect(runtime.state?.status).toBe("paused");
});

it("never invents a goal, refuses implicit replacement and validates edits", () => {
  const { host, runtime } = setup();
  vi.advanceTimersByTime(5000);
  expect(host.send).not.toHaveBeenCalled();
  expect(() => runtime.resume()).toThrow("No goal");
  runtime.start({ objective: "task", tokenBudget: null });
  expect(() =>
    runtime.start({ objective: "other", tokenBudget: null }),
  ).toThrow("unfinished goal");
  expect(() => runtime.edit(" ")).toThrow("Objective must not be empty");
  runtime.edit("revised");
  expect(runtime.state?.status).toBe("paused");
  expect(runtime.state?.objective).toBe("revised");
  runtime.resume();
  expect(runtime.state?.status).toBe("active");
});
it("sends exactly one continuation until input accepts it, rejecting stale prompts", () => {
  const { host, runtime } = setup();
  runtime.start({ objective: "task", tokenBudget: null });
  vi.advanceTimersByTime(10000);
  expect(host.send).toHaveBeenCalledTimes(1);
  const text: unknown = host.send.mock.calls[0]?.[0];
  if (typeof text !== "string") throw new Error("No continuation");
  expect(runtime.accept("invalid")).toBe(false);
  expect(runtime.accept(text)).toBe(true);
  expect(runtime.accept(text)).toBe(false);
  runtime.pause();
  vi.advanceTimersByTime(10000);
  expect(host.send).toHaveBeenCalledTimes(1);
});
it("pauses rather than repeatedly queuing rejected continuations", () => {
  const { host, runtime } = setup();
  runtime.start({ objective: "task", tokenBudget: null });
  vi.advanceTimersByTime(35000);
  expect(host.send).toHaveBeenCalledTimes(1);
  expect(runtime.state?.status).toBe("paused");
});
it("clear invalidates queued prompts and persists a tombstone", () => {
  const { host, runtime } = setup();
  runtime.start({ objective: "task", tokenBudget: null });
  vi.advanceTimersByTime(5000);
  runtime.invalidateTicket();
  runtime.clear();
  expect(runtime.state).toBeNull();
  expect(host.persist).toHaveBeenLastCalledWith("pi-goal-state-v1", null);
});
it("busy context and busy-race errors defer without failing the goal", () => {
  const { host, runtime } = setup();
  host.isReady.mockReturnValue(false);
  runtime.start({ objective: "task", tokenBudget: null });
  vi.advanceTimersByTime(5000);
  expect(host.send).not.toHaveBeenCalled();
  host.isReady.mockReturnValue(true);
  host.send.mockImplementationOnce(() => {
    throw new Error("Agent is already processing a prompt");
  });
  vi.advanceTimersByTime(5000);
  expect(runtime.state?.status).toBe("active");
  vi.advanceTimersByTime(5000);
  expect(host.send).toHaveBeenCalledTimes(2);
});
it.each([new Error("Delivery failed"), "unknown error"])(
  "pauses after non-busy delivery errors",
  (error) => {
    const { host, runtime } = setup();
    host.send.mockImplementation(() => {
      throw error;
    });
    runtime.start({ objective: "task", tokenBudget: null });
    vi.advanceTimersByTime(5000);
    expect(runtime.state?.status).toBe("paused");
  },
);
it("loop owns pacing and pending tmux delivery has priority", () => {
  const { host, runtime } = setup();
  const loop: ActivityProvider = new ActivityProvider(host.bus, () => ({
    source: "loop",
    ownsContinuation: true,
    pendingDelivery: false,
    tasks: [],
  }));
  const tmux: ActivityProvider = new ActivityProvider(host.bus, () => ({
    source: "tmux",
    ownsContinuation: false,
    pendingDelivery: true,
    tasks: [],
  }));
  loop.start("session");
  runtime.start({ objective: "task", tokenBudget: null });
  vi.advanceTimersByTime(5000);
  expect(host.send).not.toHaveBeenCalled();
  loop.stop();
  tmux.start("session");
  vi.advanceTimersByTime(5000);
  expect(host.send).not.toHaveBeenCalled();
  tmux.stop();
  vi.advanceTimersByTime(5000);
  expect(host.send).toHaveBeenCalledTimes(1);
});
it("tracks only new session-owned jobs and waits without model polling", () => {
  const { host, runtime } = setup();
  const tmux: ActivityProvider = new ActivityProvider(host.bus, () => ({
    source: "tmux",
    ownsContinuation: false,
    pendingDelivery: false,
    tasks: ["existing", "owned"],
  }));
  tmux.start("session");
  announceTask(host.bus, { sessionId: "session", name: "existing" });
  runtime.start({ objective: "task", tokenBudget: null });
  announceTask(host.bus, { sessionId: "other", name: "foreign" });
  announceTask(host.bus, { sessionId: "session", name: "owned" });
  announceTask(host.bus, { sessionId: "session", name: "owned" });
  expect(runtime.state?.tasks).toStrictEqual(["owned"]);
  vi.advanceTimersByTime(10000);
  expect(host.send).not.toHaveBeenCalled();
  expect(() => runtime.update("complete", "claimed done")).toThrow(
    "Inspect owned tmux tasks",
  );
  tmux.stop();
  vi.advanceTimersByTime(5000);
  expect(runtime.state?.status).toBe("paused");
});
it("blocks only for owned tasks whose notices are pending", () => {
  const { host, runtime } = setup();
  const tmux: ActivityProvider = new ActivityProvider(host.bus, () => ({
    source: "tmux",
    ownsContinuation: false,
    pendingDelivery: true,
    pendingTasks: ["unrelated"],
    tasks: [],
  }));
  tmux.start("session");
  runtime.start({ objective: "task", tokenBudget: null });
  announceTask(host.bus, { sessionId: "session", name: "owned" });

  expect(() => runtime.update("complete", "claimed done")).not.toThrow();
  expect(runtime.state?.status).toBe("complete");
  tmux.stop();
});
it("names the owned task that blocks completion", () => {
  const { host, runtime } = setup();
  const tmux: ActivityProvider = new ActivityProvider(host.bus, () => ({
    source: "tmux",
    ownsContinuation: false,
    pendingDelivery: true,
    pendingTasks: ["owned"],
    tasks: [],
  }));
  tmux.start("session");
  runtime.start({ objective: "task", tokenBudget: null });
  announceTask(host.bus, { sessionId: "session", name: "owned" });

  expect(() => runtime.update("complete", "claimed done")).toThrow(
    "Inspect owned tmux tasks before claiming goal completion: owned",
  );
  tmux.stop();
});
it("keeps the aggregate pending signal for publishers without per-task notices", () => {
  const { host, runtime } = setup();
  const tmux: ActivityProvider = new ActivityProvider(host.bus, () => ({
    source: "tmux",
    ownsContinuation: false,
    pendingDelivery: true,
    tasks: [],
  }));
  tmux.start("session");
  runtime.start({ objective: "task", tokenBudget: null });
  announceTask(host.bus, { sessionId: "session", name: "owned" });

  expect(() => runtime.update("complete", "claimed done")).toThrow(
    "Inspect owned tmux tasks",
  );
  tmux.stop();
});
it("unrelated pre-existing tasks do not block a goal", () => {
  const { host, runtime } = setup();
  const tmux: ActivityProvider = new ActivityProvider(host.bus, () => ({
    source: "tmux",
    ownsContinuation: false,
    pendingDelivery: false,
    tasks: ["old-watcher"],
  }));
  tmux.start("session");
  runtime.start({ objective: "task", tokenBudget: null });
  vi.advanceTimersByTime(5000);
  expect(host.send).toHaveBeenCalledTimes(1);
  tmux.stop();
});
it("validates bounded waits and wakes only at the deadline", () => {
  const { host, runtime } = setup();
  runtime.start({ objective: "task", tokenBudget: null });
  expect(() => runtime.wait({ delaySeconds: 1.5, reason: "x" })).toThrow();
  expect(() => runtime.wait({ delaySeconds: 59, reason: "x" })).toThrow();
  expect(() => runtime.wait({ delaySeconds: 3601, reason: "x" })).toThrow();
  expect(() => runtime.wait({ delaySeconds: 60, reason: " " })).toThrow();
  runtime.wait({ delaySeconds: 60, reason: "external status check" });
  vi.advanceTimersByTime(55000);
  expect(host.send).not.toHaveBeenCalled();
  vi.advanceTimersByTime(5000);
  expect(host.send).toHaveBeenCalledTimes(1);
  runtime.pause();
  expect(() => runtime.wait({ delaySeconds: 60, reason: "x" })).toThrow(
    "Goal is not active",
  );
});
it("accounts active run usage, preserves stopped states and enforces explicit budgets", () => {
  const { runtime } = setup();
  runtime.start({ objective: "task", tokenBudget: 10 });
  runtime.begin();
  runtime.recordTokens(-1);
  runtime.recordTokens(10);
  vi.advanceTimersByTime(100);
  runtime.end(null);
  runtime.settled();
  expect(runtime.state?.tokensUsed).toBe(10);
  expect(runtime.state?.elapsedMs).toBe(100);
  expect(runtime.state?.status).toBe("budget_limited");
  expect(() => runtime.resume()).toThrow("budget exhausted");
  expect(() => runtime.budget(0)).toThrow("Budget must be");
  runtime.budget(null);
  runtime.resume();
  runtime.budget(20);
  runtime.begin();
  runtime.recordTokens(100);
  runtime.end(null);
  expect(runtime.state?.tokensUsed).toBe(10);
});
it("stops on final errors but not a recovered retry", () => {
  const { runtime } = setup();
  runtime.start({ objective: "task", tokenBudget: null });
  runtime.begin();
  runtime.end("provider error");
  runtime.begin();
  runtime.end(null);
  runtime.settled();
  expect(runtime.state?.status).toBe("active");
  runtime.begin();
  runtime.end("User aborted the run.");
  runtime.settled();
  expect(runtime.state?.status).toBe("paused");
});
it("completion stops scheduling and permits a new explicit goal", () => {
  const { host, runtime } = setup();
  runtime.start({ objective: "task", tokenBudget: null });
  runtime.begin();
  runtime.update("blocked", "Evidence unavailable");
  expect(runtime.state?.status).toBe("active");
  runtime.update("complete", "Verified tests and current artifacts");
  runtime.end(null);
  runtime.settled();
  vi.advanceTimersByTime(5000);
  expect(host.send).not.toHaveBeenCalled();
  runtime.start({ objective: "next", tokenBudget: null });
  expect(runtime.state?.objective).toBe("next");
  runtime.shutdown();
  vi.advanceTimersByTime(5000);
  expect(host.send).not.toHaveBeenCalled();
});
