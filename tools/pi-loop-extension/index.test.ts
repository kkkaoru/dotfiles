// This TypeScript file is executed with Bun.
import { expect, it, vi } from "vitest";
import loopExtension, {
  type LoopCommandDefinition,
  type LoopExtensionHost,
  type LoopToolDefinition,
} from "./index.ts";
import type { LoopContext } from "./src/runtime.ts";

it("registers the loop command, tool, and lifecycle handlers", async () => {
  let command: LoopCommandDefinition | undefined;
  let tool: LoopToolDefinition | undefined;
  let onShutdown: ((event: unknown, context: LoopContext) => void) | undefined;
  let onStart: ((event: unknown, context: LoopContext) => void) | undefined;
  const sendUserMessage = vi.fn<LoopExtensionHost["sendUserMessage"]>();
  const context: LoopContext = {
    isIdle: (): boolean => true,
    ui: {
      notify: vi.fn(),
      setStatus: vi.fn(),
    },
  };
  const host: LoopExtensionHost = {
    on: (event, handler): void => {
      if (event === "session_start") {
        onStart = handler;
      } else if (event === "session_shutdown") {
        onShutdown = handler;
      }
    },
    registerCommand: (_name, definition): void => {
      command = definition;
    },
    registerTool: (definition): void => {
      tool = definition;
    },
    sendUserMessage,
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
  expect(tool?.name).toBe("loop_wakeup");
  expect(tool?.executionMode).toBe("parallel");
  command?.handler("list", context);
  expect(context.ui.notify).toHaveBeenCalledWith("No loop jobs are scheduled.", "info");

  onStart?.({}, context);
  const result = await tool?.execute(
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

  onShutdown?.({}, context);
  expect(context.ui.setStatus).toHaveBeenLastCalledWith("loop", undefined);
});

it("continues a loop through compaction and handles settled events", () => {
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
  expect(sendUserMessage).toHaveBeenLastCalledWith(
    "This is a self-paced loop. Perform the task now. Keep the session responsive: when a command is expected to run for a long time and tmux is available, start it in a detached tmux session with output and exit status redirected to files instead of waiting in the foreground. Preserve the tmux session and file paths in the next wakeup prompt, then inspect them on a later tick. Run short commands normally. Before ending, call loop_wakeup only when another useful check remains. Do not schedule another wakeup when the task is complete, blocked on user input, or waiting on external state that cannot be checked later.\n\nTask:\ncontinue work",
    {},
  );
  onSettled?.({}, context);
  onCompaction?.({ willRetry: false }, context);
  expect(sendUserMessage).toHaveBeenCalledTimes(2);
});

it("automatically detaches long-running bash calls during a loop", () => {
  let command: LoopCommandDefinition | undefined;
  let onToolCall: ((event: unknown, context: LoopContext) => void) | undefined;
  const context: LoopContext = {
    isIdle: (): boolean => true,
    ui: { notify: vi.fn(), setStatus: vi.fn() },
  };
  const host: LoopExtensionHost = {
    on: (event, handler): void => {
      if (event === "tool_call") {
        onToolCall = handler;
      }
    },
    registerCommand: (_name, definition): void => {
      command = definition;
    },
    registerTool: (): void => undefined,
    sendUserMessage: vi.fn(),
  };
  const input = { command: "gh run watch 32847265628 --exit-status --compact", timeout: 1200 };

  loopExtension(host);
  if (command === undefined || onToolCall === undefined) {
    throw new Error("Loop extension handlers were not registered");
  }
  const handleToolCall: (event: unknown, context: LoopContext) => void = onToolCall;
  command.handler("watch CI", context);
  [
    null,
    {},
    { toolName: "read" },
    { toolName: "bash" },
    { input: null, toolName: "bash" },
    { input: {}, toolName: "bash" },
    { input: { command: 42 }, toolName: "bash" },
    { input: { command: "echo ok", timeout: "slow" }, toolName: "bash" },
    { input: { command: "echo ok" }, toolName: "bash" },
  ].map((event: unknown): void => handleToolCall(event, context));
  handleToolCall({ input, toolName: "bash" }, context);
  expect(input.timeout).toBe(30);
  expect(input.command).toMatch(/tmux new-session -d/u);
});
