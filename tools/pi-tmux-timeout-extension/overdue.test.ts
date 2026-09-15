// This TypeScript file is executed with Bun.
import { afterEach, expect, it, vi } from "vitest";
import tmuxTimeoutExtension, { type TmuxExtensionHost, type TmuxToolDefinition } from "./index.ts";
import {
  CompletionDelivery,
  type CompletionDeliveryContext,
  type CompletionDeliveryHost,
} from "./src/delivery.ts";
import { createTmuxLaunch } from "./src/tmux.ts";
afterEach(() => {
  vi.useRealTimers();
});

it("delivers overdue context during an ongoing run and reconciles finished jobs first", async () => {
  vi.useFakeTimers();
  const tools: TmuxToolDefinition[] = [];
  const handlers = new Map<
    string,
    (event: unknown, context?: CompletionDeliveryContext) => unknown
  >();
  const sendUserMessage = vi.fn<TmuxExtensionHost["sendUserMessage"]>();
  const read = vi.fn().mockReturnValue("");
  tmuxTimeoutExtension(
    {
      exec: vi
        .fn<TmuxExtensionHost["exec"]>()
        .mockResolvedValue({ code: 0, stdout: "", stderr: "" }),
      on: (name, handler) => {
        handlers.set(name, handler);
      },
      registerTool: (tool) => {
        tools.push(tool);
      },
      sendUserMessage,
    },
    {
      recovery: false,
      events: { subscribe: (): (() => void) => (): void => undefined },
      operations: { read, isRunning: () => true },
    },
  );
  const context: CompletionDeliveryContext = {
    isIdle: () => false,
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  handlers.get("agent_start")?.({}, context);
  await tools[0]?.execute(
    "review",
    { command: "codex exec review", estimatedDurationSeconds: 60 },
    undefined,
  );
  vi.advanceTimersByTime(60_000);
  expect(sendUserMessage).not.toHaveBeenCalled();
  expect(handlers.get("context")?.({ messages: [] }, context)).toStrictEqual({
    messages: [expect.objectContaining({ customType: "tmux-overdue" })],
  });
  const launch = await tools[0]?.execute(
    "second",
    { command: "bounded check", estimatedDurationSeconds: 60 },
    undefined,
  );
  vi.advanceTimersByTime(60_000);
  handlers.get("tool_result")?.({
    toolName: "read",
    isError: false,
    input: { path: launch?.details.logPath },
  });
  expect(handlers.get("context")?.({ messages: [] }, context)).toStrictEqual({
    messages: [
      expect.objectContaining({
        content: expect.stringMatching(/^tmux overdue check-in: 1 task/u),
      }),
    ],
  });
  vi.advanceTimersByTime(300_000);
  read.mockReturnValue("0\n");
  expect(handlers.get("context")?.({ messages: [] }, context)).toBeUndefined();
  handlers.get("session_shutdown")?.({});
});

it("retains deduplicated notices across model calls until the log is read", () => {
  const sendUserMessage = vi.fn<CompletionDeliveryHost["sendUserMessage"]>();
  const delivery = new CompletionDelivery({ sendUserMessage });
  const context: CompletionDeliveryContext = {
    isIdle: () => false,
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  const launch = createTmuxLaunch({
    command: "codex exec review",
    id: 1,
    namespace: "a".repeat(32),
  });
  delivery.setContext(context);
  delivery.overdue([launch]);
  delivery.overdue([launch]);
  const event = { messages: [{ role: "user", content: "continue" }] };
  expect(delivery.injectOverdue(event)).toStrictEqual({
    messages: [
      { role: "user", content: "continue" },
      expect.objectContaining({
        role: "custom",
        customType: "tmux-overdue",
        content: expect.stringMatching(/^tmux overdue check-in: 1 task\(s\)/u),
      }),
    ],
  });
  expect(event.messages).toStrictEqual([{ role: "user", content: "continue" }]);
  expect(delivery.injectOverdue(event)?.messages).toHaveLength(2);
  expect(delivery.hasPending()).toBe(true);
  delivery.inspectedLog({ toolName: "read", input: { path: launch.logPath }, isError: false });
  expect(delivery.injectOverdue(event)).toBeUndefined();
  expect(delivery.hasPending()).toBe(false);
  expect(sendUserMessage).not.toHaveBeenCalled();
});

it("retains context notices during compaction and excludes jobs completed before injection", () => {
  const delivery = new CompletionDelivery({ sendUserMessage: vi.fn() });
  const context: CompletionDeliveryContext = {
    isIdle: () => false,
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  const launch = createTmuxLaunch({ command: "review", id: 1, namespace: "a".repeat(32) });
  delivery.beforeCompaction(context);
  delivery.overdue([launch]);
  expect(delivery.injectOverdue({ messages: [] })).toBeUndefined();
  delivery.afterCompaction(context);
  expect(delivery.injectOverdue(null)).toBeUndefined();
  expect(delivery.injectOverdue("invalid")).toBeUndefined();
  expect(delivery.injectOverdue({})).toBeUndefined();
  expect(delivery.injectOverdue({ messages: "invalid" })).toBeUndefined();
  expect(delivery.hasPending()).toBe(true);
  delivery.complete({ launch, completedAt: new Date().toISOString(), exitCode: 0 });
  expect(delivery.injectOverdue({ messages: [] })).toBeUndefined();
});

it("rotates bounded busy batches without losing uninspected jobs", () => {
  const delivery = new CompletionDelivery({ sendUserMessage: vi.fn() });
  delivery.setContext({
    isIdle: () => false,
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  });
  delivery.overdue(
    Array.from({ length: 21 }, (_value, id) =>
      createTmuxLaunch({ command: "review", id, namespace: "a".repeat(32) }),
    ),
  );
  expect(delivery.injectOverdue({ messages: [] })?.messages).toStrictEqual([
    expect.objectContaining({ content: expect.stringMatching(/^tmux overdue check-in: 20 task/u) }),
  ]);
  expect(delivery.injectOverdue({ messages: [] })?.messages).toStrictEqual([
    expect.objectContaining({ content: expect.stringMatching(/^tmux overdue check-in: 20 task/u) }),
  ]);
  expect(delivery.hasPending()).toBe(true);
  delivery.clear();
  expect(delivery.hasPending()).toBe(false);
});
