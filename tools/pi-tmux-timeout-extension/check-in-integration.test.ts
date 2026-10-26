// This TypeScript file is executed with Bun.
import { afterEach, expect, it, vi } from "vitest";
import {
  registerTmux as tmuxTimeoutExtension,
  type TmuxExtensionHost,
  type TmuxToolDefinition,
} from "./index.ts";
import type { CompletionDeliveryContext } from "./src/delivery.ts";
import { TMUX_SESSION_ENTRY_TYPE } from "./src/persistence.ts";

interface Fixture {
  readonly appendEntry: ReturnType<typeof vi.fn>;
  readonly context: CompletionDeliveryContext;
  readonly handlers: Map<string, (event: unknown, context?: CompletionDeliveryContext) => unknown>;
  readonly sendUserMessage: ReturnType<typeof vi.fn>;
  readonly setWidget: ReturnType<typeof vi.fn>;
  readonly tools: readonly TmuxToolDefinition[];
}

function fixture(): Fixture {
  const tools: TmuxToolDefinition[] = [];
  const handlers = new Map<
    string,
    (event: unknown, context?: CompletionDeliveryContext) => unknown
  >();
  const sendUserMessage = vi.fn();
  const appendEntry = vi.fn();
  const setWidget = vi.fn();
  const context: CompletionDeliveryContext = {
    isIdle: () => true,
    sessionManager: { getEntries: () => [], getSessionId: () => "test-session" },
    ui: { notify: vi.fn(), setStatus: vi.fn(), setWidget },
  };
  const host: TmuxExtensionHost = {
    appendEntry,
    exec: vi.fn<TmuxExtensionHost["exec"]>().mockResolvedValue({ code: 0, stdout: "", stderr: "" }),
    on: (name, handler) => {
      handlers.set(name, handler);
    },
    registerTool: (tool) => {
      tools.push(tool);
    },
    sendUserMessage,
  };
  tmuxTimeoutExtension(host, {
    recovery: false,
    events: { subscribe: (): (() => void) => (): void => undefined },
    operations: { read: () => "", isRunning: () => true },
  });
  handlers.get("session_start")?.({}, context);
  return { appendEntry, context, handlers, sendUserMessage, setWidget, tools };
}

afterEach(() => {
  vi.useRealTimers();
});

it("resolves the overdue row through tmux_check_in and schedules the next check", async () => {
  vi.useFakeTimers();
  const test = fixture();
  const started = await test.tools[0]?.execute(
    "overdue",
    { command: "python3 watch.py", estimatedDurationSeconds: 60 },
    undefined,
  );
  vi.advanceTimersByTime(60_000);
  expect(test.setWidget).toHaveBeenLastCalledWith("tmux-running-tasks", [
    expect.stringMatching(/^⚠ overdue/u),
  ]);
  const checkIn = test.tools.find((tool) => tool.name === "tmux_check_in");
  if (checkIn === undefined || started === undefined) {
    throw new Error("Expected tmux_exec and tmux_check_in tools");
  }

  const result = await checkIn.execute(
    "check-in",
    { nextCheckSeconds: 600, sessionName: started.details.sessionName },
    undefined,
  );

  expect(result.content[0].text).toMatch(/^tmux check-in recorded/u);
  expect(test.appendEntry).toHaveBeenCalledWith(
    TMUX_SESSION_ENTRY_TYPE,
    expect.objectContaining({ sessionName: started.details.sessionName }),
  );
  expect(test.setWidget).toHaveBeenCalledWith("tmux-running-tasks", [
    expect.stringMatching(/^⏳/u),
  ]);
  expect(
    test.handlers.get("context_with_system")?.({ messages: [] }, test.context),
  ).toBeUndefined();
  vi.advanceTimersByTime(599_000);
  expect(test.sendUserMessage).toHaveBeenCalledOnce();
  vi.advanceTimersByTime(1000);
  expect(test.sendUserMessage).toHaveBeenCalledTimes(2);
  test.handlers.get("session_shutdown")?.({});
});
