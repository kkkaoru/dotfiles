// This TypeScript file is executed with Bun.
import { afterEach, expect, it, vi } from "vitest";
import loopExtension, {
  type LoopCommandDefinition,
  type LoopExtensionHost,
  type LoopToolDefinition,
} from "./index.ts";
import type { LoopContext } from "./src/runtime.ts";

afterEach((): void => {
  vi.useRealTimers();
});

it("registers the loop command, tools, and shutdown handler", () => {
  let command: LoopCommandDefinition | undefined;
  const tools: LoopToolDefinition[] = [];
  let onShutdown: ((event: unknown, context: LoopContext) => void) | undefined;
  const context: LoopContext = {
    isIdle: (): boolean => true,
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  const host: LoopExtensionHost = {
    on: (event, handler): void => {
      if (event === "session_shutdown") {
        onShutdown = handler;
      }
    },
    registerCommand: (_name, definition): void => {
      command = definition;
    },
    registerTool: (definition): void => {
      tools.push(definition);
    },
    sendUserMessage: vi.fn(),
  };

  loopExtension(host);
  expect(command?.description).toBe(
    "Run a prompt now and continue on a self-paced or fixed schedule",
  );
  expect(command?.getArgumentCompletions("")).toStrictEqual([
    { label: "list", value: "list" },
    { label: "clear", value: "clear" },
    { label: "pause", value: "pause" },
    { label: "resume", value: "resume" },
    { label: "5m ", value: "5m " },
    { label: "30m ", value: "30m " },
    { label: "1h ", value: "1h " },
  ]);
  expect(command?.getArgumentCompletions("unknown")).toBeNull();
  expect(tools.map((tool: LoopToolDefinition): string => tool.name)).toStrictEqual([
    "loop_wakeup",
    "loop_complete",
  ]);
  const [wakeupTool] = tools;
  expect(wakeupTool?.executionMode).toBe("parallel");
  command?.handler("list", context);
  expect(context.ui.notify).toHaveBeenCalledWith("No loop jobs are scheduled.", "info");
  onShutdown?.({}, context);
  expect(context.ui.setStatus).toHaveBeenLastCalledWith("loop", undefined);
});

it("schedules a wakeup through the registered tool after session start", async () => {
  let command: LoopCommandDefinition | undefined;
  const tools: LoopToolDefinition[] = [];
  let onStart: ((event: unknown, context: LoopContext) => void) | undefined;
  const context: LoopContext = {
    isIdle: (): boolean => true,
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  const host: LoopExtensionHost = {
    on: (event, handler): void => {
      if (event === "session_start") {
        onStart = handler;
      }
    },
    registerCommand: (_name, definition): void => {
      command = definition;
    },
    registerTool: (definition): void => {
      tools.push(definition);
    },
    sendUserMessage: vi.fn(),
  };

  loopExtension(host);
  onStart?.({}, context);
  command?.handler("check", context);
  const [wakeupTool] = tools;
  if (wakeupTool?.name !== "loop_wakeup") {
    throw new Error("loop_wakeup was not registered first");
  }
  const result = await wakeupTool.execute(
    "call-1",
    { delaySeconds: 60, prompt: "check", reason: "pending" },
    undefined,
    undefined,
    context,
  );

  expect(result).toStrictEqual({
    content: [{ text: "Scheduled loop #1 in 60 seconds.", type: "text" }],
    details: { id: 1, scheduledInSeconds: 60 },
  });
});

it("completes a self-paced loop through the registered tool", async () => {
  let command: LoopCommandDefinition | undefined;
  const tools: LoopToolDefinition[] = [];
  const sendUserMessage = vi.fn<LoopExtensionHost["sendUserMessage"]>();
  const context: LoopContext = {
    isIdle: (): boolean => true,
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  const host: LoopExtensionHost = {
    on: (): void => undefined,
    registerCommand: (_name, definition): void => {
      command = definition;
    },
    registerTool: (definition): void => {
      tools.push(definition);
    },
    sendUserMessage,
  };

  loopExtension(host);
  command?.handler("finish work", context);
  const [, completeTool] = tools;
  if (completeTool?.name !== "loop_complete") {
    throw new Error("loop_complete was not registered second");
  }
  const result = await completeTool.execute(
    "call-2",
    { reason: "all checks passed" },
    undefined,
    undefined,
    context,
  );

  expect(result).toStrictEqual({
    content: [{ text: "Completed loop: all checks passed", type: "text" }],
    details: { reason: "all checks passed" },
  });
});

it("continues a loop after the settled event has returned", () => {
  vi.useFakeTimers();
  let command: LoopCommandDefinition | undefined;
  let onCompaction: ((event: unknown, context: LoopContext) => void) | undefined;
  let onSettled: ((event: unknown, context: LoopContext) => void) | undefined;
  const sendUserMessage = vi.fn<LoopExtensionHost["sendUserMessage"]>();
  const context: LoopContext = {
    isIdle: (): boolean => true,
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  const host: LoopExtensionHost = {
    on: (event, handler): void => {
      if (event === "session_compact") {
        onCompaction = handler;
      } else if (event === "agent_settled") {
        onSettled = handler;
      }
    },
    registerCommand: (_name, definition): void => {
      command = definition;
    },
    registerTool: (): void => undefined,
    sendUserMessage,
  };

  loopExtension(host);
  command?.handler("continue work", context);
  onCompaction?.({ willRetry: false }, context);
  expect(sendUserMessage).toHaveBeenCalledOnce();
  vi.runOnlyPendingTimers();
  expect(sendUserMessage).toHaveBeenLastCalledWith(
    expect.stringMatching(
      /^\d{2}-\d{2} \d{2}:\d{2} → \d{2}:\d{2} \| loop=self-paced \| continue work\nThis is a self-paced loop\./u,
    ),
    { deliverAs: "followUp" },
  );
  onSettled?.({}, context);
  expect(sendUserMessage).toHaveBeenCalledTimes(2);
  vi.runOnlyPendingTimers();
  expect(sendUserMessage).toHaveBeenCalledTimes(3);
  onCompaction?.({ willRetry: false }, context);
  vi.runOnlyPendingTimers();
  expect(sendUserMessage).toHaveBeenCalledTimes(4);
});
