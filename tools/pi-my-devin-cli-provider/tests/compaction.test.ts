// This file runs with Bun.
import { expect, test, vi } from "vitest";
import { handleDevinCompactionEvent, registerDevinCompaction } from "../src/compaction.ts";

const mocks = vi.hoisted(() => ({
  invalidateDevinSessionsForPiSession: vi.fn(),
}));

vi.mock("../src/runtime.ts", () => ({
  invalidateDevinSessionsForPiSession: mocks.invalidateDevinSessionsForPiSession,
}));

test("registers session compaction hooks that invalidate Devin sessions", () => {
  const events: string[] = [];
  const runners: Array<(event: object, ctx: object) => unknown> = [];

  registerDevinCompaction({
    on(event, handler) {
      events.push(event);
      runners.push((evt, ctx) => Reflect.apply(handler, undefined, [evt, ctx]));
    },
  });

  expect(events).toStrictEqual(["session_before_compact", "session_compact"]);
  const ctx = {
    model: { provider: "devin" },
    sessionManager: { getSessionId: () => "pi-session-1" },
  };
  runners.forEach((run) => {
    run({ type: "compact" }, ctx);
  });
  expect(mocks.invalidateDevinSessionsForPiSession).toHaveBeenCalledTimes(2);
  expect(mocks.invalidateDevinSessionsForPiSession).toHaveBeenCalledWith("pi-session-1");
});

test("skips invalidation when the active model is not Devin", () => {
  handleDevinCompactionEvent({
    model: { provider: "openai" },
    sessionManager: { getSessionId: () => "other-session" },
  });
  handleDevinCompactionEvent({
    model: undefined,
    sessionManager: { getSessionId: () => "no-model" },
  });
  expect(mocks.invalidateDevinSessionsForPiSession).not.toHaveBeenCalled();
});
