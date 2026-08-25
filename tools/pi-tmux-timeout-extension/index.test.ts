// This TypeScript file is executed with Bun.
import { expect, it, vi } from "vitest";
import tmuxTimeoutExtension, {
  type TmuxExtensionHost,
  type TmuxToolDefinition,
  wakePiOnCompletion,
} from "./index.ts";

it("registers and executes the parallel tmux tool", async () => {
  let tool: TmuxToolDefinition | undefined;
  const exec = vi.fn<TmuxExtensionHost["exec"]>().mockResolvedValue({
    code: 0,
    stderr: "",
    stdout: "started",
  });
  const host: TmuxExtensionHost = {
    exec,
    on: (): void => undefined,
    registerTool: (definition): void => {
      tool = definition;
    },
    sendUserMessage: vi.fn(),
  };

  tmuxTimeoutExtension(host, {
    events: { subscribe: (): (() => void) => (): void => undefined },
  });
  expect(tool?.name).toBe("tmux_exec");
  expect(tool?.executionMode).toBe("parallel");
  const controller = new globalThis.AbortController();
  const result = await tool?.execute("call-1", { command: "sleep 60" }, controller.signal);
  expect(exec).toHaveBeenCalledOnce();
  expect(result?.content[0]?.text).toMatch(/Started detached tmux command/u);
  expect(result?.details.sessionName).toMatch(/^pi-tmux-\d+-1$/u);
  expect(result?.details.logPath).toMatch(/output\.log$/u);
  expect(result?.details.statusPath).toMatch(/exit-status$/u);
});

it("reports tmux launch failures", async () => {
  let tool: TmuxToolDefinition | undefined;
  const host: TmuxExtensionHost = {
    exec: vi.fn<TmuxExtensionHost["exec"]>().mockResolvedValue({
      code: 1,
      stderr: "tmux unavailable",
      stdout: "",
    }),
    on: (): void => undefined,
    registerTool: (definition): void => {
      tool = definition;
    },
    sendUserMessage: vi.fn(),
  };

  tmuxTimeoutExtension(host, {
    events: { subscribe: (): (() => void) => (): void => undefined },
  });
  await expect(tool?.execute("call-1", { command: "sleep 60" }, undefined)).rejects.toThrow(
    "tmux unavailable",
  );
});

it("falls back through stdout and a generic tmux launch error", async () => {
  let tool: TmuxToolDefinition | undefined;
  const exec = vi
    .fn<TmuxExtensionHost["exec"]>()
    .mockResolvedValueOnce({ code: 1, stderr: "", stdout: "tmux stdout failure" })
    .mockResolvedValueOnce({ code: 1, stderr: "", stdout: "" });
  const host: TmuxExtensionHost = {
    exec,
    on: (): void => undefined,
    registerTool: (definition): void => {
      tool = definition;
    },
    sendUserMessage: vi.fn(),
  };

  tmuxTimeoutExtension(host, {
    events: { subscribe: (): (() => void) => (): void => undefined },
  });
  await expect(tool?.execute("call-1", { command: "sleep 60" }, undefined)).rejects.toThrow(
    "tmux stdout failure",
  );
  await expect(tool?.execute("call-2", { command: "sleep 60" }, undefined)).rejects.toThrow(
    "Failed to start tmux command",
  );
});

it("wakes pi immediately when completion monitoring reports an exit status", () => {
  const sendUserMessage = vi.fn<TmuxExtensionHost["sendUserMessage"]>();
  const host: TmuxExtensionHost = {
    exec: vi.fn(),
    on: (): void => undefined,
    registerTool: (): void => undefined,
    sendUserMessage,
  };

  wakePiOnCompletion(host, {
    exitCode: 0,
    launch: {
      command: "tmux command",
      completionChannel: "pi-tmux-test-complete",
      logPath: "/tmp/pi-tmux-test/output.log",
      sessionName: "pi-tmux-test",
      statusPath: "/tmp/pi-tmux-test/exit-status",
    },
  });
  expect(sendUserMessage).toHaveBeenCalledWith(
    expect.stringMatching(/exit code: 0[\s\S]*Do not wait for a previously scheduled timeout/u),
    { deliverAs: "followUp" },
  );
});

it("automatically rewrites and event-subscribes successful long-running bash calls", () => {
  let onShutdown: ((event: unknown) => void) | undefined;
  let onToolCall: ((event: unknown) => void) | undefined;
  let onToolResult: ((event: unknown) => void) | undefined;
  const subscribe = vi.fn((): (() => void) => (): void => undefined);
  const host: TmuxExtensionHost = {
    exec: vi.fn(),
    on: (event, handler): void => {
      if (event === "tool_call") {
        onToolCall = handler;
      } else if (event === "tool_result") {
        onToolResult = handler;
      } else {
        onShutdown = handler;
      }
    },
    registerTool: (): void => undefined,
    sendUserMessage: vi.fn(),
  };
  const input = { command: "gh run watch 32847265628 --exit-status --compact", timeout: 1200 };
  const failedInput = { command: "tail -f server.log", timeout: 1200 };

  tmuxTimeoutExtension(host, { events: { subscribe } });
  if (onToolCall === undefined || onToolResult === undefined) {
    throw new Error("Tmux extension tool handlers were not registered");
  }
  const handleToolCall: (event: unknown) => void = onToolCall;
  const handleToolResult: (event: unknown) => void = onToolResult;
  [
    null,
    {},
    { toolName: "read" },
    { toolName: "bash" },
    { input: null, toolCallId: "bad-1", toolName: "bash" },
    { input: {}, toolCallId: "bad-2", toolName: "bash" },
    { input: { command: 42 }, toolCallId: "bad-3", toolName: "bash" },
    { input: { command: "echo ok", timeout: "slow" }, toolCallId: "bad-4", toolName: "bash" },
    { input: { command: "echo ok" }, toolCallId: "short", toolName: "bash" },
  ].map((event: unknown): void => handleToolCall(event));
  [
    { input, toolCallId: "call-1", toolName: "bash" },
    { input: failedInput, toolCallId: "call-2", toolName: "bash" },
  ].map((event: unknown): void => handleToolCall(event));
  [
    {},
    { isError: false, toolCallId: "missing" },
    { isError: false, toolCallId: "call-1" },
    { isError: true, toolCallId: "call-2" },
  ].map((event: unknown): void => handleToolResult(event));
  expect(input.timeout).toBe(30);
  expect(input.command).toMatch(/tmux new-session -d/u);
  expect(subscribe).toHaveBeenCalledOnce();
  onShutdown?.({});
});
