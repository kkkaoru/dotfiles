// This TypeScript file is executed with Bun.
import { describe, expect, it } from "vitest";
import { announceTask, type ActivityBus, type ActivitySnapshot } from "./goal-activity.ts";
import { liveTaskNames, pruneWatchedTasks, WatchedLoopTasks } from "./watched-tasks.ts";

function tmux(tasks: readonly string[], pendingTasks?: readonly string[]): ActivitySnapshot {
  return pendingTasks === undefined
    ? { ownsContinuation: false, pendingDelivery: false, source: "tmux", tasks }
    : { ownsContinuation: false, pendingDelivery: false, pendingTasks, source: "tmux", tasks };
}

function testBus(): ActivityBus {
  const listeners = new Map<string, Set<(data: unknown) => void>>();
  return {
    emit: (channel: string, data: unknown): void => {
      const channelListeners: Set<(data: unknown) => void> | undefined = listeners.get(channel);
      if (channelListeners === undefined) {
        return;
      }
      for (const listener of channelListeners) {
        listener(data);
      }
    },
    on: (channel: string, listener: (data: unknown) => void): (() => void) => {
      const channelListeners: Set<(data: unknown) => void> =
        listeners.get(channel) ?? new Set<(data: unknown) => void>();
      channelListeners.add(listener);
      listeners.set(channel, channelListeners);
      return (): void => {
        channelListeners.delete(listener);
      };
    },
  };
}

describe("pruneWatchedTasks", () => {
  it("keeps live and pending names and drops finished ones", () => {
    const pruned = pruneWatchedTasks(new Set(["live", "pending", "done"]), [
      tmux(["live", "other"], ["pending"]),
    ]);
    expect(pruned).toStrictEqual(new Set(["live", "pending"]));
  });

  it("empties watched names when no snapshot reports them", () => {
    expect(pruneWatchedTasks(new Set(["old"]), [])).toStrictEqual(new Set());
    expect(pruneWatchedTasks(new Set(["old"]), [tmux([])])).toStrictEqual(new Set());
  });

  it("collects live names across loop and tmux snapshots", () => {
    expect(
      liveTaskNames([
        { ownsContinuation: true, pendingDelivery: false, source: "loop", tasks: [] },
        tmux(["a"], ["b"]),
      ]),
    ).toStrictEqual(new Set(["a", "b"]));
  });
});

describe("WatchedLoopTasks", () => {
  it("stays quiet without a bus, session, or tracked launches", () => {
    const watched = new WatchedLoopTasks();
    expect(watched.unfinished(undefined)).toBe(false);
    const bus = testBus();
    expect(watched.unfinished(bus)).toBe(false);
    watched.attach(undefined, "session", () => true);
    expect(watched.unfinished(bus)).toBe(false);
    watched.attach(bus, undefined, () => true);
    expect(watched.unfinished(bus)).toBe(false);
    watched.detach();
  });

  it("tracks launches from a running tick and prunes finished work", () => {
    const bus = testBus();
    let running = true;
    const watched = new WatchedLoopTasks();
    watched.attach(bus, "session", () => running);
    announceTask(bus, { name: "render", sessionId: "session" });
    announceTask(bus, { name: "other-session", sessionId: "elsewhere" });
    running = false;
    announceTask(bus, { name: "idle-launch", sessionId: "session" });
    // Only the running-tick launch is tracked, and "render" already finished.
    // Other live names reported by the provider are untracked, so this is false.
    bus.on("pi:goal:activity-query:v1", (request: unknown): void => {
      const query = request as {
        sessionId: string;
        respond: (snapshot: ActivitySnapshot) => void;
      };
      if (query.sessionId === "session") {
        query.respond(tmux(["other-session", "idle-launch"]));
      }
    });
    expect(watched.unfinished(bus)).toBe(false);
    expect(watched.unfinished(undefined)).toBe(false);
    watched.clear();
    watched.detach();
  });

  it("reports unfinished work while a tracked launch stays live", () => {
    const bus = testBus();
    const watched = new WatchedLoopTasks();
    watched.attach(bus, "session", () => true);
    announceTask(bus, { name: "render", sessionId: "session" });
    let live = true;
    bus.on("pi:goal:activity-query:v1", (request: unknown): void => {
      const query = request as {
        sessionId: string;
        respond: (snapshot: ActivitySnapshot) => void;
      };
      if (query.sessionId === "session") {
        query.respond(tmux(live ? ["render"] : []));
      }
    });
    expect(watched.unfinished(bus)).toBe(true);
    live = false;
    expect(watched.unfinished(bus)).toBe(false);
    watched.detach();
  });
});
