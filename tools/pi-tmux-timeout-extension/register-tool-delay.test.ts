// This TypeScript file is executed with Bun.
import { expect, it, vi } from "vitest";
import {
  registerTmux as tmuxTimeoutExtension,
  type TmuxExtensionHost,
  type TmuxToolDefinition,
} from "./index.ts";

it("refuses a sleep-only command before launching anything", async () => {
  let tool: TmuxToolDefinition | undefined;
  const exec = vi.fn<TmuxExtensionHost["exec"]>();
  const host: TmuxExtensionHost = {
    exec,
    on: (): void => undefined,
    registerTool: (definition): void => {
      if (definition.name === "tmux_exec") {
        tool = definition;
      }
    },
    sendUserMessage: vi.fn(),
  };

  tmuxTimeoutExtension(host, {
    events: { subscribe: (): (() => void) => (): void => undefined },
  });
  await expect(tool?.execute("call-1", { command: "sleep 600; date" }, undefined)).rejects.toThrow(
    /loop_wakeup/u,
  );
  expect(exec).not.toHaveBeenCalled();
});

it("refuses to auto-detach a bash call that starts with sleep", async () => {
  let onToolCall: ((event: unknown) => unknown) | undefined;
  const exec = vi.fn<TmuxExtensionHost["exec"]>();
  const host: TmuxExtensionHost = {
    exec,
    on: (event, handler): void => {
      if (event === "tool_call") {
        onToolCall = handler;
      }
    },
    registerTool: (): void => undefined,
    sendUserMessage: vi.fn(),
  };
  tmuxTimeoutExtension(host, {
    events: { subscribe: (): (() => void) => (): void => undefined },
  });
  const input = { command: "cd /tmp && sleep 600; date" };

  await expect(onToolCall?.({ input, toolCallId: "call-1", toolName: "bash" })).rejects.toThrow(
    /loop_wakeup/u,
  );
  expect(exec).not.toHaveBeenCalled();
  expect(input.command).toBe("cd /tmp && sleep 600; date");
});
