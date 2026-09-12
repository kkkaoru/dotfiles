// This TypeScript file is executed with Bun.
import { afterEach, expect, it, vi } from "vitest";
import { queryActivity, type ActivityBus } from "../../pi-goal-extension/src/activity.ts";
import loopExtension, { type LoopCommandDefinition, type LoopExtensionHost } from "../index.ts";
import { LoopRuntime, type LoopContext } from "./runtime.ts";
import { createLoopState } from "./state.ts";

afterEach(() => {
  vi.useRealTimers();
});

it("exposes scoped loop ownership and removes the provider on shutdown", () => {
  vi.useFakeTimers();
  const emitter: EventTarget = new globalThis.EventTarget();
  const events: ActivityBus = {
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
  const callbacks = new Map<string, (event: unknown, context: LoopContext) => void>();
  const commands: LoopCommandDefinition[] = [];
  const context: LoopContext = {
    isIdle: () => true,
    sessionManager: { getEntries: () => [], getSessionId: () => "session" },
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  const host: LoopExtensionHost = {
    events,
    sendUserMessage: vi.fn(),
    registerTool: vi.fn(),
    on: (name, callback) => {
      callbacks.set(name, callback);
    },
    registerCommand: (_name, definition) => {
      commands.push(definition);
    },
  };
  loopExtension(host);
  callbacks.get("session_start")?.({}, context);
  expect(queryActivity(events, "session")).toStrictEqual([
    { source: "loop", ownsContinuation: false, pendingDelivery: false, tasks: [] },
  ]);
  commands[0]?.handler("task", context);
  expect(queryActivity(events, "session")[0]?.ownsContinuation).toBe(true);
  commands[0]?.handler("pause", context);
  expect(queryActivity(events, "session")[0]?.ownsContinuation).toBe(false);
  commands[0]?.handler("clear", context);
  commands[0]?.handler("5m task", context);
  expect(queryActivity(events, "session")[0]?.ownsContinuation).toBe(true);
  expect(queryActivity(events, "foreign")).toStrictEqual([]);
  callbacks.get("session_shutdown")?.({}, context);
  expect(queryActivity(events, "session")).toStrictEqual([]);
});
it("pending restored continuations also own pacing", () => {
  vi.useFakeTimers();
  const runtime: LoopRuntime = new LoopRuntime({ sendUserMessage: vi.fn() });
  const context: LoopContext = { isIdle: () => false, ui: { notify: vi.fn(), setStatus: vi.fn() } };
  runtime.restore(
    createLoopState({
      jobs: [],
      nextId: 1,
      paused: false,
      pendingContinuations: ["pending"],
      runningContinuation: undefined,
    }),
    context,
  );
  expect(runtime.ownsContinuation()).toBe(true);
  runtime.shutdown();
});
