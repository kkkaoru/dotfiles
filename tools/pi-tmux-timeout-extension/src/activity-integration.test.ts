// This TypeScript file is executed with Bun. Process and filesystem operations are mocked.
import { afterEach, expect, it, vi, type Mock } from "vitest";
import {
  queryActivity,
  subscribeTasks,
  type ActivityBus,
} from "../../pi-goal-extension/src/activity.ts";
import tmuxTimeoutExtension, { type TmuxExtensionHost, type TmuxToolDefinition } from "../index.ts";
import type { CompletionDeliveryContext } from "./delivery.ts";

interface Harness {
  readonly events: ActivityBus;
  readonly idle: Mock<() => boolean>;
  readonly read: Mock<() => string>;
  readonly notice: Mock;
  readonly start: () => void;
  readonly stop: () => void;
  readonly settled: () => void;
  readonly launch: () => Promise<unknown>;
}

afterEach(() => {
  vi.useRealTimers();
});

function createBus(): ActivityBus {
  const emitter: EventTarget = new globalThis.EventTarget();
  return {
    emit: (channel, value) => {
      emitter.dispatchEvent(new globalThis.CustomEvent<unknown>(channel, { detail: value }));
    },
    on: (channel, listener) => {
      const handler = (event: Event): void => {
        if (event instanceof globalThis.CustomEvent) {
          listener(event.detail);
        }
      };
      emitter.addEventListener(channel, handler);
      return () => {
        emitter.removeEventListener(channel, handler);
      };
    },
  };
}
function setup(): Harness {
  const events: ActivityBus = createBus();
  const callbacks = new Map<
    string,
    (event: unknown, context?: CompletionDeliveryContext) => void
  >();
  const tools: TmuxToolDefinition[] = [];
  const idle = vi.fn(() => false);
  const read = vi.fn(() => "");
  const notice = vi.fn();
  const remove = subscribeTasks(events, notice);
  const context: CompletionDeliveryContext = {
    isIdle: idle,
    sessionManager: { getEntries: () => [], getSessionId: () => "session" },
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  const host: TmuxExtensionHost = {
    events,
    sendUserMessage: vi.fn(),
    exec: vi.fn<TmuxExtensionHost["exec"]>().mockResolvedValue({ code: 0, stdout: "", stderr: "" }),
    on: (name, callback) => {
      callbacks.set(name, callback);
    },
    registerTool: (tool) => {
      tools.push(tool);
    },
  };
  tmuxTimeoutExtension(host, {
    recovery: false,
    events: { subscribe: (): (() => void) => (): void => undefined },
    operations: { read, isRunning: () => true },
  });
  return {
    events,
    idle,
    read,
    notice,
    start: () => {
      callbacks.get("session_start")?.({}, context);
    },
    stop: () => {
      callbacks.get("session_shutdown")?.({}, context);
      remove();
    },
    settled: () => {
      callbacks.get("agent_settled")?.({}, context);
    },
    launch: async () =>
      tools[0]?.execute("call", { command: "test", estimatedDurationSeconds: 60 }, undefined),
  };
}
it("exposes only session-scoped launches and disposes providers", async () => {
  vi.useFakeTimers();
  const harness: Harness = setup();
  await harness.launch();
  expect(harness.notice).not.toHaveBeenCalled();
  harness.start();
  expect(queryActivity(harness.events, "session")[0]?.pendingDelivery).toBe(false);
  await harness.launch();
  expect(harness.notice).toHaveBeenCalledWith(
    expect.objectContaining({ sessionId: "session", name: expect.stringMatching(/^pi-tmux-/u) }),
  );
  expect(queryActivity(harness.events, "session")[0]?.tasks).toHaveLength(1);
  expect(queryActivity(harness.events, "session")[0]?.pendingTasks).toStrictEqual([]);
  expect(queryActivity(harness.events, "foreign")).toStrictEqual([]);
  harness.stop();
  expect(queryActivity(harness.events, "session")).toStrictEqual([]);
});
it("reports overdue and completed work pending delivery, not false completion", async () => {
  vi.useFakeTimers();
  const harness: Harness = setup();
  harness.start();
  const launched = (await harness.launch()) as { details: { sessionName: string } };
  await vi.advanceTimersByTimeAsync(60_000);
  expect(queryActivity(harness.events, "session")[0]?.pendingDelivery).toBe(true);
  expect(queryActivity(harness.events, "session")[0]?.pendingTasks).toStrictEqual([
    launched.details.sessionName,
  ]);
  harness.read.mockReturnValue("0");
  await vi.advanceTimersByTimeAsync(60_000);
  expect(queryActivity(harness.events, "session")[0]?.tasks).toStrictEqual([]);
  expect(queryActivity(harness.events, "session")[0]?.pendingDelivery).toBe(true);
  expect(queryActivity(harness.events, "session")[0]?.pendingTasks).toStrictEqual([
    launched.details.sessionName,
  ]);
  harness.idle.mockReturnValue(true);
  harness.settled();
  await vi.advanceTimersByTimeAsync(0);
  expect(queryActivity(harness.events, "session")[0]?.pendingDelivery).toBe(false);
  expect(queryActivity(harness.events, "session")[0]?.pendingTasks).toStrictEqual([]);
  harness.stop();
});
