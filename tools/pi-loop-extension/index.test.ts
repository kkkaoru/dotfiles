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
  let onStart: ((event: unknown, context: LoopContext) => void) | undefined;
  let onShutdown: ((event: unknown, context: LoopContext) => void) | undefined;
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
      } else {
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
