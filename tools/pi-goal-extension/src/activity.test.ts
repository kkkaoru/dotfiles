// Runs with Bun; event bus is synchronous and entirely in memory.
import { EventEmitter } from "node:events";
import { expect, it, vi } from "vitest";
import { ActivityProvider as LoopProvider } from "../../pi-loop-extension/src/goal-activity.ts";
import { announceTask as announceFromTmux } from "../../pi-tmux-timeout-extension/src/goal-activity.ts";
import {
  type ActivityBus,
  ActivityProvider,
  announceTask,
  queryActivity,
  subscribeTasks,
} from "./activity.ts";

function bus(): ActivityBus {
  const emitter: EventEmitter = new EventEmitter();
  return {
    emit: (channel, data) => {
      emitter.emit(channel, data);
    },
    on: (channel, listener) => {
      emitter.on(channel, listener);
      return () => {
        emitter.off(channel, listener);
      };
    },
  };
}

it("shares the protocol through extension-local source links", () => {
  const events: ActivityBus = bus();
  const listener = vi.fn();
  const remove = subscribeTasks(events, listener);
  const provider: LoopProvider = new LoopProvider(events, () => ({
    source: "loop",
    ownsContinuation: true,
    pendingDelivery: false,
    tasks: [],
  }));
  provider.start("session");
  expect(queryActivity(events, "session")[0]?.ownsContinuation).toBe(true);
  announceFromTmux(events, { sessionId: "session", name: "task" });
  expect(listener).toHaveBeenCalledWith({ sessionId: "session", name: "task" });
  provider.stop();
  remove();
});

it("queries fresh session-scoped snapshots without retaining old providers", () => {
  const events: ActivityBus = bus();
  const provider: ActivityProvider = new ActivityProvider(events, () => ({
    source: "loop",
    ownsContinuation: true,
    pendingDelivery: false,
    tasks: [],
  }));
  expect(queryActivity(events, "session")).toStrictEqual([]);
  provider.start("session");
  expect(queryActivity(events, "other")).toStrictEqual([]);
  expect(queryActivity(events, "session")).toStrictEqual([
    {
      source: "loop",
      ownsContinuation: true,
      pendingDelivery: false,
      tasks: [],
    },
  ]);
  provider.start("session");
  expect(queryActivity(events, "session")).toHaveLength(1);
  provider.stop();
  expect(queryActivity(events, "session")).toStrictEqual([]);
  provider.stop();
});
it.each([null, {}, { sessionId: 1 }, { sessionId: "session", respond: false }])(
  "ignores malformed activity request",
  (value) => {
    const events: ActivityBus = bus();
    const snapshot = vi.fn(
      () =>
        ({
          source: "loop",
          ownsContinuation: false,
          pendingDelivery: false,
          tasks: [],
        }) satisfies import("./activity.ts").ActivitySnapshot,
    );
    const provider: ActivityProvider = new ActivityProvider(events, snapshot);
    provider.start("session");
    events.emit("pi:goal:activity-query:v1", value);
    expect(snapshot).not.toHaveBeenCalled();
    provider.stop();
  },
);
it("delivers valid launch notices and disposes listeners", () => {
  const events: ActivityBus = bus();
  const listener = vi.fn();
  const remove = subscribeTasks(events, listener);
  announceTask(events, { sessionId: "session", name: "task-1" });
  expect(listener).toHaveBeenCalledWith({
    sessionId: "session",
    name: "task-1",
  });
  remove();
  announceTask(events, { sessionId: "session", name: "task-2" });
  expect(listener).toHaveBeenCalledTimes(1);
});
it.each([null, {}, { sessionId: 1 }, { sessionId: "s", name: 1 }])(
  "ignores malformed launch notice",
  (value) => {
    const events: ActivityBus = bus();
    const listener = vi.fn();
    const remove = subscribeTasks(events, listener);
    events.emit("pi:goal:task-launch:v1", value);
    expect(listener).not.toHaveBeenCalled();
    remove();
  },
);
