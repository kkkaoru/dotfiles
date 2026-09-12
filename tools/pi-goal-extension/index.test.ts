// Runs with Bun; Pi callbacks, editor, session I/O and model delivery are mocked.
import { EventEmitter } from "node:events";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import goalExtension, { type GoalExtensionHost } from "./index.ts";

interface Harness {
  readonly pi: ReturnType<typeof fakePi>;
  readonly context: ReturnType<typeof fakeContext>;
  call(name: string, event: unknown): Promise<unknown>;
  command(args: string): Promise<unknown>;
  tool(name: string, params: unknown): Promise<unknown>;
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(1000);
});
afterEach(() => {
  vi.clearAllTimers();
  vi.useRealTimers();
});

function fakePi() {
  const emitter: EventEmitter = new EventEmitter();
  return {
    events: {
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
    on: vi.fn<(event: string, handler: unknown) => void>(),
    registerCommand: vi.fn<GoalExtensionHost["registerCommand"]>(),
    registerTool: vi.fn<(definition: unknown) => void>(),
    sendUserMessage: vi.fn<GoalExtensionHost["sendUserMessage"]>(),
    appendEntry: vi.fn<GoalExtensionHost["appendEntry"]>(),
  } satisfies GoalExtensionHost;
}
function fakeContext() {
  return {
    sessionManager: { getSessionId: () => "session", getBranch: () => [] },
    isIdle: vi.fn(() => true),
    hasPendingMessages: vi.fn(() => false),
    ui: {
      setStatus: vi.fn(),
      notify: vi.fn(),
      editor: vi.fn<() => Promise<string | undefined>>(),
    },
  };
}
async function invoke(callback: unknown, ...args: unknown[]): Promise<unknown> {
  if (typeof callback !== "function") throw new Error("Missing callback");
  return await callback(...args);
}
function field(value: unknown, key: string): unknown {
  return typeof value === "object" && value !== null
    ? Reflect.get(value, key)
    : undefined;
}
function harness(): Harness {
  const pi = fakePi();
  const context = fakeContext();
  goalExtension(pi);
  return {
    pi,
    context,
    call: (name, event) => {
      const calls: readonly (readonly unknown[])[] = pi.on.mock.calls;
      return invoke(
        calls.find((call) => call[0] === name)?.[1],
        event,
        context,
      );
    },
    command: (args) =>
      invoke(pi.registerCommand.mock.calls[0]?.[1].handler, args, context),
    tool: (name, params) =>
      invoke(
        field(
          pi.registerTool.mock.calls.find(
            (call) => field(call[0], "name") === name,
          )?.[0],
          "execute",
        ),
        "call",
        params,
      ),
  };
}

it("registers explicit goal commands and tools without creating a goal", async () => {
  const h: Harness = harness();
  await h.command("task");
  expect(h.context.ui.notify).toHaveBeenCalledWith(
    "Goal session is not initialized.",
    "error",
  );
  await h.call("session_start", {});
  await h.command("status");
  h.pi.appendEntry.mockImplementationOnce(() => {
    throw "storage unavailable";
  });
  await h.command("task");
  expect(h.context.ui.notify).toHaveBeenLastCalledWith(
    "Goal command failed.",
    "error",
  );
  expect(await h.tool("get_goal", {})).toMatchObject({
    details: { goal: null },
  });
  expect(
    await h.call("before_agent_start", { systemPrompt: "base" }),
  ).toBeUndefined();
  await h.call("input", { source: "extension", text: "ordinary notification" });
  await h.call("session_shutdown", {});
  await h.call("agent_settled", {});
  await h.call("agent_start", {});
  await h.call("message_end", { message: {} });
  await h.call("tool_execution_end", { toolName: "read" });
  await h.call("agent_end", { messages: [] });
  await h.call("before_agent_start", { systemPrompt: "base" });
  await h.call("input", { source: "user", text: "ordinary input" });
  expect(
    await h.call("input", {
      source: "extension",
      text: "[pi-goal-continuation:stale]",
    }),
  ).toStrictEqual({ action: "handled" });
});
it("creates, injects goal guidance, accepts only current tickets and audits completion", async () => {
  const h: Harness = harness();
  await h.call("session_start", {});
  await h.command("task");
  await vi.advanceTimersByTimeAsync(5000);
  const text: unknown = h.pi.sendUserMessage.mock.calls[0]?.[0];
  expect(await h.call("input", { source: "extension", text })).toStrictEqual({
    action: "continue",
  });
  expect(
    await h.call("before_agent_start", { systemPrompt: "base" }),
  ).toMatchObject({
    systemPrompt: expect.stringMatching(/base\n\nThe following JSON/),
  });
  await h.call("agent_start", {});
  await h.call("tool_execution_end", { toolName: "read" });
  await h.call("message_end", { message: { usage: { totalTokens: 10 } } });
  await h.call("agent_end", {
    messages: [{ role: "assistant", stopReason: "stop" }],
  });
  await h.call("agent_settled", {});
  expect(await h.tool("get_goal", {})).toMatchObject({
    details: { goal: { tokensUsed: 10 } },
  });
  await h.tool("update_goal", {
    status: "complete",
    reason: "Verified artifacts",
  });
  expect(await h.tool("get_goal", {})).toMatchObject({
    details: { goal: { status: "complete" } },
  });
  await h.call("session_shutdown", {});
});
it("user controls stop scheduling, change budgets and clear goals", async () => {
  const h: Harness = harness();
  await h.call("session_start", {});
  await h.command("task");
  await h.command("pause");
  await h.command("budget 100");
  await h.command("edit revised");
  await h.command("resume");
  await h.tool("goal_wait", { delaySeconds: 60, reason: "External check" });
  expect(await h.tool("get_goal", {})).toMatchObject({
    details: { goal: { objective: "revised", tokenBudget: 100 } },
  });
  await h.call("input", { source: "user", text: "other work" });
  await h.command("clear");
  expect(await h.tool("get_goal", {})).toMatchObject({
    details: { goal: null },
  });
  await h.command("--bad");
  expect(h.context.ui.notify).toHaveBeenLastCalledWith(
    expect.stringMatching(/\/goal \[/),
    "error",
  );
});
it("editor preserves cancellation and detects concurrent session changes", async () => {
  const h: Harness = harness();
  await h.call("session_start", {});
  await h.command("task");
  h.context.ui.editor.mockResolvedValueOnce(undefined);
  await h.command("edit");
  expect(await h.tool("get_goal", {})).toMatchObject({
    details: { goal: { objective: "task", status: "paused" } },
  });
  h.context.ui.editor.mockResolvedValueOnce("edited");
  await h.command("edit");
  expect(await h.tool("get_goal", {})).toMatchObject({
    details: { goal: { objective: "edited" } },
  });
  h.context.ui.editor.mockImplementationOnce(async () => {
    await h.call("session_tree", {});
    return "stale";
  });
  await h.command("edit");
  expect(h.context.ui.notify).toHaveBeenLastCalledWith(
    "Goal changed while the editor was open; retry editing.",
    "error",
  );
});
it("compaction, blocking UI and pending user input prevent competing delivery", async () => {
  const h: Harness = harness();
  await h.call("session_start", {});
  await h.command("task");
  await h.call("session_before_compact", {});
  await vi.advanceTimersByTimeAsync(5000);
  expect(h.pi.sendUserMessage).not.toHaveBeenCalled();
  await h.call("session_compact", {
    compactionEntry: { usage: { totalTokens: 5 } },
  });
  await h.call("ui_prompt_start", {});
  await vi.advanceTimersByTimeAsync(5000);
  expect(h.pi.sendUserMessage).not.toHaveBeenCalled();
  await h.call("ui_prompt_end", {});
  h.context.hasPendingMessages.mockReturnValue(true);
  await vi.advanceTimersByTimeAsync(5000);
  expect(h.pi.sendUserMessage).not.toHaveBeenCalled();
  h.context.hasPendingMessages.mockReturnValue(false);
  await h.call("session_compact_failed", {});
  await vi.advanceTimersByTimeAsync(5000);
  expect(h.pi.sendUserMessage).toHaveBeenCalledTimes(1);
  expect(await h.tool("get_goal", {})).toMatchObject({
    details: { goal: { tokensUsed: 5 } },
  });
  await h.call("session_shutdown", {});
});
