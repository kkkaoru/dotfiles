// This TypeScript file is executed with Bun.
import { afterEach, expect, it, vi } from "vitest";
import loopExtension, { type LoopCommandDefinition, type LoopExtensionHost } from "../index.ts";
import {
  failedAgentRun,
  pauseAfterAgentFailure,
  pauseAfterCompactionFailure,
} from "./compaction-failure.ts";
import { LoopRuntime, type LoopContext } from "./runtime.ts";

const context: LoopContext = {
  isIdle: () => true,
  ui: { notify: vi.fn(), setStatus: vi.fn() },
};

afterEach(() => vi.useRealTimers());

it("pauses failed compaction durably and does not continue until explicit resume", () => {
  vi.useFakeTimers();
  const sendUserMessage = vi.fn();
  const appendEntry = vi.fn();
  const runtime = new LoopRuntime({ sendUserMessage, appendEntry });
  runtime.command("finish the task", context);
  runtime.deferLifecycleContinuation(() => runtime.agentSettled(context));
  pauseAfterCompactionFailure(runtime, context);
  pauseAfterCompactionFailure(runtime, context);
  runtime.agentSettled(context);
  runtime.continueAfterCompaction(false, context);
  vi.advanceTimersByTime(60_000);
  expect(sendUserMessage).toHaveBeenCalledTimes(1);
  expect(runtime.ownsContinuation()).toBe(false);
  expect(appendEntry.mock.lastCall?.[1]).toMatchObject({ paused: true });
  runtime.command("resume", context);
  vi.advanceTimersByTime(1);
  expect(sendUserMessage).toHaveBeenCalledTimes(2);
  expect(runtime.ownsContinuation()).toBe(true);
  runtime.shutdown();
});

it("does not pause an unrelated session and retains messages while busy", () => {
  const sendUserMessage = vi.fn();
  const runtime = new LoopRuntime({ sendUserMessage });
  pauseAfterCompactionFailure(runtime, context);
  expect(context.ui.notify).not.toHaveBeenCalled();
  pauseAfterAgentFailure(runtime, context);
  runtime.command("finish", context);
  runtime.agentSettled({ ...context, isIdle: () => false });
  expect(sendUserMessage).toHaveBeenCalledTimes(1);
  pauseAfterAgentFailure(runtime, context);
  expect(runtime.ownsContinuation()).toBe(false);
  runtime.shutdown();
});

it("wires the failure lifecycle event and stops recurring timers", () => {
  vi.useFakeTimers();
  const handlers = new Map<string, (event: unknown, ctx: LoopContext) => void>();
  const commands: LoopCommandDefinition[] = [];
  const sendUserMessage = vi.fn();
  const host: LoopExtensionHost = {
    on: (event, handler) => {
      handlers.set(event, handler);
    },
    registerCommand: (_name, command) => {
      commands.push(command);
    },
    registerTool: () => undefined,
    sendUserMessage,
  };
  loopExtension(host);
  commands[0]?.handler("1m check", context);
  handlers.get("session_compact_failed")?.({ aborted: false }, context);
  handlers.get("agent_settled")?.({}, context);
  vi.advanceTimersByTime(120_000);
  expect(sendUserMessage).toHaveBeenCalledTimes(1);
  commands[0]?.handler("resume", context);
  handlers.get("agent_end")?.({ messages: [{ role: "assistant", stopReason: "error" }] }, context);
  handlers.get("agent_settled")?.({}, context);
  vi.advanceTimersByTime(120_000);
  expect(sendUserMessage).toHaveBeenCalledTimes(1);
  handlers.get("session_shutdown")?.({}, context);
});

it("allows successful native retries before settlement", () => {
  vi.useFakeTimers();
  const handlers = new Map<string, (event: unknown, ctx: LoopContext) => void>();
  const commands: LoopCommandDefinition[] = [];
  const sendUserMessage = vi.fn();
  loopExtension({
    on: (event, handler) => {
      handlers.set(event, handler);
    },
    registerCommand: (_name, command) => {
      commands.push(command);
    },
    registerTool: () => undefined,
    sendUserMessage,
  });
  commands[0]?.handler("finish", context);
  handlers.get("agent_end")?.({ messages: [{ role: "assistant", stopReason: "error" }] }, context);
  handlers.get("agent_end")?.({ messages: [{ role: "assistant", stopReason: "stop" }] }, context);
  handlers.get("agent_settled")?.({}, context);
  vi.advanceTimersByTime(1);
  expect(sendUserMessage).toHaveBeenCalledTimes(2);
  handlers.get("session_shutdown")?.({}, context);
});

it.each([
  null,
  4,
  {},
  { messages: null },
  { messages: [] },
  { messages: [null, 2, {}, { role: "user" }, { role: "assistant" }] },
  { messages: [{ role: "assistant", stopReason: "stop" }] },
])("ignores non-error agent events: %j", (event) => {
  expect(failedAgentRun(event)).toBe(false);
});

it.each(["error", "aborted"])("recognizes terminal assistant %s", (stopReason) => {
  expect(
    failedAgentRun({ messages: [{ role: "assistant", stopReason }, { role: "toolResult" }] }),
  ).toBe(true);
});
