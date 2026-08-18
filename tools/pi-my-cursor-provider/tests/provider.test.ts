import type {
  AgentOptions,
  InteractionUpdate,
  Run,
  RunResult,
  SDKAgent,
  SendOptions,
  ToolCall,
} from "@cursor/sdk";
import type {
  Api,
  AssistantMessageEvent,
  AssistantMessageEventStream,
  Context,
  Model,
  ToolResultMessage,
} from "@earendil-works/pi-ai";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

interface FakeScenario {
  onSend(options: SendOptions | undefined, agentOptions: AgentOptions): Promise<RunResult>;
}

const createAgentMock = vi.fn<(options: AgentOptions) => Promise<SDKAgent>>();

vi.mock("@cursor/sdk", async (importOriginal) => {
  const original = await importOriginal<typeof import("@cursor/sdk")>();
  return {
    ...original,
    Agent: { create: createAgentMock },
  };
});

const MODEL: Model<Api> = {
  id: "auto",
  name: "Cursor Auto",
  api: "cursor-agent",
  provider: "cursor",
  baseUrl: "https://cursor.com",
  reasoning: false,
  input: ["text", "image"],
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
  contextWindow: 200_000,
  maxTokens: 32_000,
};

function result(text: string): RunResult {
  return {
    id: "run-1",
    status: "finished",
    result: text,
    usage: {
      inputTokens: 10,
      outputTokens: 2,
      cacheReadTokens: 1,
      cacheWriteTokens: 0,
      totalTokens: 12,
    },
  };
}

function fakeRun(wait: () => Promise<RunResult>): Run {
  return {
    id: "run-1",
    agentId: "agent-1",
    status: "running",
    supports: () => true,
    unsupportedReason: () => undefined,
    stream: async function* () {},
    conversation: async () => [],
    wait,
    cancel: async () => undefined,
    onDidChangeStatus: () => () => undefined,
  };
}

function installScenario(scenario: FakeScenario): void {
  createAgentMock.mockImplementation(async (agentOptions) => ({
    agentId: "agent-1",
    model: agentOptions.model,
    send: async (_message, options) => fakeRun(() => scenario.onSend(options, agentOptions)),
    close: () => undefined,
    reload: async () => undefined,
    [Symbol.asyncDispose]: async () => undefined,
    listArtifacts: async () => [],
    downloadArtifact: async () => Buffer.alloc(0),
    getUsage: async () => ({
      usage: {
        inputTokens: 0,
        outputTokens: 0,
        cacheReadTokens: 0,
        cacheWriteTokens: 0,
        totalTokens: 0,
      },
      runs: [],
    }),
  }));
}

async function collect(stream: AssistantMessageEventStream): Promise<AssistantMessageEvent[]> {
  const events: AssistantMessageEvent[] = [];
  for await (const event of stream) events.push(event);
  return events;
}

function baseContext(): Context {
  return {
    systemPrompt: "Be exact.",
    messages: [{ role: "user", content: "Say hello", timestamp: 1 }],
  };
}

beforeEach(async () => {
  createAgentMock.mockReset();
  const { cursorPlatformTestApi } = await import("../src/platform.ts");
  cursorPlatformTestApi.reset();
  cursorPlatformTestApi.setEnabled(false);
});

afterEach(async () => {
  const { cursorProviderTestApi } = await import("../src/provider.ts");
  const { cursorStoreTestApi } = await import("../src/store.ts");
  const { cursorPlatformTestApi } = await import("../src/platform.ts");
  cursorProviderTestApi.setClaimTimeoutMs();
  await cursorProviderTestApi.waitForIdle();
  cursorStoreTestApi.reset();
  cursorPlatformTestApi.reset();
  expect(cursorProviderTestApi.pendingCount()).toBe(0);
});

test("streams Cursor text with usage through a fresh agent", async () => {
  const captured: { sendOptions: SendOptions | undefined } = { sendOptions: undefined };
  installScenario({
    async onSend(options) {
      captured.sendOptions = options;
      await options?.onDelta?.({ update: { type: "text-delta", text: "hello" } });
      await options?.onDelta?.({
        update: {
          type: "turn-ended",
          usage: {
            inputTokens: 10,
            outputTokens: 2,
            cacheReadTokens: 1,
            cacheWriteTokens: 0,
          },
        },
      });
      return result("hello");
    },
  });
  const { streamCursor } = await import("../src/provider.ts");

  const events = await collect(streamCursor(MODEL, baseContext(), { apiKey: "key" }));

  expect(events.map((event) => event.type)).toStrictEqual([
    "start",
    "text_start",
    "text_delta",
    "done",
  ]);
  const done = events.at(-1);
  expect(done?.type === "done" ? done.message.content : []).toStrictEqual([
    { type: "text", text: "hello" },
  ]);
  expect(done?.type === "done" ? done.message.usage.totalTokens : -1).toBe(12);
  expect(createAgentMock).toHaveBeenCalledTimes(1);
  const created = createAgentMock.mock.calls[0]?.[0];
  expect(created).toMatchObject({
    apiKey: "key",
    agentId: expect.stringMatching(new RegExp(`^pi-cursor-${process.pid}-[0-9a-f-]+$`)),
    name: expect.stringMatching(new RegExp(`^pi-cursor-${process.pid}-[0-9a-f-]+$`)),
    model: { id: "auto" },
    tools: ["mcp", "webSearch", "semSearch", "shell"],
    local: { cwd: process.cwd(), settingSources: [], customTools: {} },
  });
  expect(created?.local?.store).toBeDefined();
  expect(captured.sendOptions?.local).toStrictEqual({ force: true });
});

test("forwards explicit-model effort with the live default variant", async () => {
  installScenario({
    async onSend() {
      return result("hello");
    },
  });
  const { recordCursorCatalog } = await import("../src/models.ts");
  recordCursorCatalog([
    {
      id: "claude-sonnet-4-6",
      displayName: "Sonnet 4.6",
      parameters: [
        {
          id: "effort",
          values: ["low", "medium", "high", "max"].map((value) => ({ value })),
        },
      ],
      variants: [
        {
          displayName: "Default",
          isDefault: true,
          params: [
            { id: "thinking", value: "true" },
            { id: "context", value: "1m" },
            { id: "effort", value: "medium" },
          ],
        },
      ],
    },
  ]);
  const { streamCursor } = await import("../src/provider.ts");
  const explicitModel = { ...MODEL, id: "claude-sonnet-4-6", name: "Sonnet 4.6" };

  await collect(streamCursor(explicitModel, baseContext(), { apiKey: "key", reasoning: "max" }));

  expect(createAgentMock.mock.calls[0]?.[0].model).toStrictEqual({
    id: "claude-sonnet-4-6",
    params: [
      { id: "thinking", value: "true" },
      { id: "context", value: "1m" },
      { id: "effort", value: "max" },
    ],
  });
});

test("bridges custom tools and continues the same live run with its result", async () => {
  const captured: { toolResult: unknown } = { toolResult: undefined };
  installScenario({
    async onSend(options, agentOptions) {
      const tool = agentOptions.local?.customTools?.["read"];
      if (!tool) throw new Error("missing read tool");
      const pending = tool.execute({ path: "README.md" }, { toolCallId: "call-1" });
      captured.toolResult = await pending;
      await options?.onDelta?.({ update: { type: "text-delta", text: "done" } });
      return result("done");
    },
  });
  const { streamCursor } = await import("../src/provider.ts");
  const context: Context = {
    ...baseContext(),
    tools: [
      {
        name: "read",
        description: "Read a file",
        parameters: {
          type: "object",
          properties: { path: { type: "string" } },
          required: ["path"],
        },
      },
    ],
  };

  const firstEvents = await collect(streamCursor(MODEL, context, { apiKey: "key" }));
  expect(firstEvents.map((event) => event.type)).toStrictEqual([
    "start",
    "toolcall_start",
    "toolcall_delta",
    "toolcall_end",
    "done",
  ]);
  const firstDone = firstEvents.at(-1);
  expect(firstDone?.type === "done" ? firstDone.reason : "").toBe("toolUse");
  const toolCallEnd = firstEvents.find((event) => event.type === "toolcall_end");
  const toolCallId = toolCallEnd?.type === "toolcall_end" ? toolCallEnd.toolCall.id : "";
  expect(toolCallId).toMatch(/^cursor-tool-[0-9a-f-]+$/);

  const toolMessage: ToolResultMessage = {
    role: "toolResult",
    toolCallId,
    toolName: "read",
    content: [{ type: "text", text: "file contents" }],
    isError: false,
    timestamp: 2,
  };
  const secondEvents = await collect(
    streamCursor(
      MODEL,
      { ...context, messages: [...context.messages, toolMessage] },
      { apiKey: "key" },
    ),
  );

  expect(secondEvents.map((event) => event.type)).toStrictEqual([
    "start",
    "text_start",
    "text_delta",
    "done",
  ]);
  expect(captured.toolResult).toStrictEqual({
    content: [{ type: "text", text: "file contents" }],
    isError: false,
  });
  expect(createAgentMock).toHaveBeenCalledTimes(1);
});

test("does not resume or reuse completed agents across requests", async () => {
  installScenario({
    async onSend() {
      return result("fallback text");
    },
  });
  const { streamCursor } = await import("../src/provider.ts");

  const first = await collect(streamCursor(MODEL, baseContext()));
  const second = await collect(streamCursor(MODEL, baseContext()));

  expect(first.at(-1)?.type).toBe("done");
  expect(second.at(-1)?.type).toBe("done");
  expect(createAgentMock).toHaveBeenCalledTimes(2);
  const firstDone = first.at(-1);
  expect(firstDone?.type === "done" ? firstDone.message.content : []).toStrictEqual([
    { type: "text", text: "fallback text" },
  ]);
});

test("reports terminal Cursor failures", async () => {
  installScenario({
    async onSend() {
      return {
        id: "run-error",
        status: "error",
        error: { message: "provider unavailable" },
      };
    },
  });
  const { streamCursor } = await import("../src/provider.ts");

  const events = await collect(streamCursor(MODEL, baseContext()));

  expect(events.map((event) => event.type)).toStrictEqual(["start", "error"]);
  const error = events.at(-1);
  expect(error?.type === "error" ? error.error.errorMessage : "").toBe("provider unavailable");
});

test("aborts the active Cursor run", async () => {
  const controller = new AbortController();
  const waitGate: { finish: ((value: RunResult) => void) | undefined } = { finish: undefined };
  const wait = new Promise<RunResult>((resolve) => {
    waitGate.finish = resolve;
  });
  const cancel = vi.fn(async () => {
    waitGate.finish?.({ id: "run-1", status: "cancelled" });
  });
  createAgentMock.mockResolvedValue({
    agentId: "agent-1",
    model: { id: "auto" },
    send: async () => ({ ...fakeRun(() => wait), cancel }),
    close: () => undefined,
    reload: async () => undefined,
    [Symbol.asyncDispose]: async () => undefined,
    listArtifacts: async () => [],
    downloadArtifact: async () => Buffer.alloc(0),
    getUsage: async () => ({
      usage: {
        inputTokens: 0,
        outputTokens: 0,
        cacheReadTokens: 0,
        cacheWriteTokens: 0,
        totalTokens: 0,
      },
      runs: [],
    }),
  });
  const { streamCursor } = await import("../src/provider.ts");
  const stream = streamCursor(MODEL, baseContext(), { signal: controller.signal });

  await new Promise<void>((resolve) => setImmediate(resolve));
  controller.abort();
  const events = await collect(stream);

  expect(cancel).toHaveBeenCalledTimes(1);
  expect(events.map((event) => event.type)).toStrictEqual(["start", "error"]);
  const error = events.at(-1);
  expect(error?.type === "error" ? error.reason : "").toBe("aborted");
});

test("waits for the previous agent to dispose before starting a follow-up request", async () => {
  const firstGate: { finish: ((value: RunResult) => void) | undefined } = { finish: undefined };
  const firstWait = new Promise<RunResult>((resolve) => {
    firstGate.finish = resolve;
  });
  const disposeGate: { release: (() => void) | undefined } = { release: undefined };
  const firstDispose = new Promise<void>((resolve) => {
    disposeGate.release = resolve;
  });
  const created: { second: boolean } = { second: false };

  createAgentMock
    .mockResolvedValueOnce({
      agentId: "agent-1",
      model: { id: "auto" },
      send: async () => ({
        ...fakeRun(() => firstWait),
        cancel: async () => {
          firstGate.finish?.({ id: "run-1", status: "cancelled" });
        },
      }),
      close: () => undefined,
      reload: async () => undefined,
      [Symbol.asyncDispose]: async () => firstDispose,
      listArtifacts: async () => [],
      downloadArtifact: async () => Buffer.alloc(0),
      getUsage: async () => ({
        usage: {
          inputTokens: 0,
          outputTokens: 0,
          cacheReadTokens: 0,
          cacheWriteTokens: 0,
          totalTokens: 0,
        },
        runs: [],
      }),
    })
    .mockImplementationOnce(async () => {
      created.second = true;
      return {
        agentId: "agent-2",
        model: { id: "auto" },
        send: async () => fakeRun(async () => result("second")),
        close: () => undefined,
        reload: async () => undefined,
        [Symbol.asyncDispose]: async () => undefined,
        listArtifacts: async () => [],
        downloadArtifact: async () => Buffer.alloc(0),
        getUsage: async () => ({
          usage: {
            inputTokens: 0,
            outputTokens: 0,
            cacheReadTokens: 0,
            cacheWriteTokens: 0,
            totalTokens: 0,
          },
          runs: [],
        }),
      };
    });

  const { streamCursor } = await import("../src/provider.ts");
  const firstController = new AbortController();
  const firstStream = streamCursor(MODEL, baseContext(), { signal: firstController.signal });
  await new Promise<void>((resolve) => setImmediate(resolve));
  firstController.abort();
  const secondPromise = collect(streamCursor(MODEL, baseContext()));
  await new Promise<void>((resolve) => setImmediate(resolve));

  expect(createAgentMock).toHaveBeenCalledTimes(1);
  expect(created.second).toBe(false);

  disposeGate.release?.();
  const [firstEvents, secondEvents] = await Promise.all([collect(firstStream), secondPromise]);

  expect(createAgentMock).toHaveBeenCalledTimes(2);
  expect(created.second).toBe(true);
  expect(firstEvents.at(-1)?.type).toBe("error");
  expect(secondEvents.at(-1)?.type).toBe("done");
});

test("does not wait forever when the previous agent dispose hangs", async () => {
  const firstWait = new Promise<RunResult>(() => undefined);
  createAgentMock
    .mockResolvedValueOnce({
      agentId: "agent-1",
      model: { id: "auto" },
      send: async () => fakeRun(() => firstWait),
      close: () => undefined,
      reload: async () => undefined,
      [Symbol.asyncDispose]: () => new Promise(() => undefined),
      listArtifacts: async () => [],
      downloadArtifact: async () => Buffer.alloc(0),
      getUsage: async () => ({
        usage: {
          inputTokens: 0,
          outputTokens: 0,
          cacheReadTokens: 0,
          cacheWriteTokens: 0,
          totalTokens: 0,
        },
        runs: [],
      }),
    })
    .mockResolvedValueOnce({
      agentId: "agent-2",
      model: { id: "auto" },
      send: async () => fakeRun(async () => result("second")),
      close: () => undefined,
      reload: async () => undefined,
      [Symbol.asyncDispose]: async () => undefined,
      listArtifacts: async () => [],
      downloadArtifact: async () => Buffer.alloc(0),
      getUsage: async () => ({
        usage: {
          inputTokens: 0,
          outputTokens: 0,
          cacheReadTokens: 0,
          cacheWriteTokens: 0,
          totalTokens: 0,
        },
        runs: [],
      }),
    });

  const { cursorProviderTestApi, streamCursor } = await import("../src/provider.ts");
  cursorProviderTestApi.setClaimTimeoutMs(20);
  const firstController = new AbortController();
  streamCursor(MODEL, baseContext(), { signal: firstController.signal });
  await new Promise<void>((resolve) => setImmediate(resolve));
  const secondEvents = await collect(streamCursor(MODEL, baseContext()));

  expect(createAgentMock).toHaveBeenCalledTimes(2);
  expect(secondEvents.at(-1)?.type).toBe("done");
  firstController.abort();
});

test("uses the terminal status when Cursor provides no error detail", async () => {
  installScenario({
    async onSend() {
      return { id: "run-cancelled", status: "cancelled" };
    },
  });
  const { streamCursor } = await import("../src/provider.ts");

  const events = await collect(streamCursor(MODEL, baseContext()));

  const error = events.at(-1);
  expect(error?.type === "error" ? error.error.errorMessage : "").toBe(
    "Cursor run ended with status cancelled",
  );
});

test("streams thinking separately from text", async () => {
  installScenario({
    async onSend(options) {
      const updates: InteractionUpdate[] = [
        { type: "thinking-delta", text: "consider" },
        { type: "thinking-delta", text: "ing" },
        { type: "thinking-completed", thinkingDurationMs: 1 },
        { type: "text-delta", text: "answer" },
      ];
      for (const update of updates) await options?.onDelta?.({ update });
      return result("answer");
    },
  });
  const { streamCursor } = await import("../src/provider.ts");

  const events = await collect(streamCursor(MODEL, baseContext()));

  expect(events.map((event) => event.type)).toStrictEqual([
    "start",
    "thinking_start",
    "thinking_delta",
    "thinking_delta",
    "thinking_end",
    "text_start",
    "text_delta",
    "done",
  ]);
});

test("streams additional Cursor update types", async () => {
  installScenario({
    async onSend(options) {
      const updates: InteractionUpdate[] = [
        { type: "shell-output-delta", event: { text: "shell chunk" } },
        { type: "text-delta", text: "hello " },
        { type: "thinking-delta", text: "think" },
        { type: "thinking-completed", thinkingDurationMs: 1 },
        { type: "summary", summary: "Working on it" },
        { type: "summary-completed" },
        { type: "step-started", stepId: 1 },
        { type: "step-completed", stepId: 1, stepDurationMs: 100 },
        {
          type: "tool-call-started",
          callId: "shell-1",
          modelCallId: "mc",
          toolCall: { type: "shell", args: { command: "ls" } },
        },
        {
          type: "tool-call-completed",
          callId: "shell-1",
          modelCallId: "mc",
          toolCall: {
            type: "shell",
            args: { command: "ls" },
            result: {
              status: "success",
              value: {
                exitCode: 0,
                signal: "SIGTERM",
                stdout: "out",
                stderr: "err",
                executionTime: 1,
              },
            },
          },
        },
        {
          type: "tool-call-completed",
          callId: "edit-1",
          modelCallId: "mc",
          toolCall: {
            type: "edit",
            args: { path: "x" },
            result: { status: "success", value: { diffString: "+added" } },
          },
        },
        {
          type: "tool-call-completed",
          callId: "edit-2",
          modelCallId: "mc",
          toolCall: {
            type: "edit",
            args: { path: "y" },
            result: {
              status: "success",
              value: { diffString: "--- a/y\n+++ b/y\n-removed" },
            },
          },
        },
        {
          type: "tool-call-completed",
          callId: "read-1",
          modelCallId: "mc",
          toolCall: {
            type: "read",
            args: { path: "r" },
            result: { status: "success", value: { content: "file", totalLines: 1, fileSize: 4 } },
          },
        },
        {
          type: "tool-call-completed",
          callId: "write-1",
          modelCallId: "mc",
          toolCall: {
            type: "write",
            args: { path: "w", fileText: "new" },
            result: {
              status: "success",
              value: { path: "w", linesCreated: 1, fileSize: 3, fileContentAfterWrite: "new" },
            },
          },
        },
        {
          type: "tool-call-completed",
          callId: "delete-1",
          modelCallId: "mc",
          toolCall: {
            type: "delete",
            args: { path: "d" },
            result: { status: "success", value: { fileSize: 5 } },
          },
        },
        {
          type: "tool-call-completed",
          callId: "glob-1",
          modelCallId: "mc",
          toolCall: {
            type: "glob",
            args: { globPattern: "*.ts" },
            result: {
              status: "success",
              value: {
                files: ["a.ts", "b.ts"],
                totalFiles: 2,
                clientTruncated: true,
                ripgrepTruncated: false,
              },
            },
          },
        },
        {
          type: "tool-call-completed",
          callId: "mcp-1",
          modelCallId: "mc",
          toolCall: {
            type: "mcp",
            args: { providerIdentifier: "p", toolName: "t" },
            result: {
              status: "success",
              value: { content: [{ text: { text: "mcp result" } }], isError: false },
            },
          },
        },
        {
          type: "tool-call-completed",
          callId: "mcp-2",
          modelCallId: "mc",
          toolCall: {
            type: "mcp",
            args: { providerIdentifier: "p" },
            result: {
              status: "success",
              value: { content: [{ image: { data: "data" } }], isError: true },
            },
          },
        },
        {
          type: "tool-call-completed",
          callId: "plan-1",
          modelCallId: "mc",
          toolCall: {
            type: "createPlan",
            args: { plan: "do it" },
            result: { status: "success", value: {} },
          },
        },
        {
          type: "tool-call-completed",
          callId: "error-1",
          modelCallId: "mc",
          toolCall: {
            type: "shell",
            args: { command: "false" },
            result: { status: "error", error: new Error("failed") },
          },
        },
        {
          type: "tool-call-started",
          callId: "dup-start",
          modelCallId: "mc",
          toolCall: { type: "shell", args: { command: "ls" } },
        },
        {
          type: "tool-call-started",
          callId: "dup-start",
          modelCallId: "mc",
          toolCall: { type: "shell", args: { command: "ls" } },
        },
        {
          type: "tool-call-completed",
          callId: "dup-complete",
          modelCallId: "mc",
          toolCall: {
            type: "createPlan",
            args: { plan: "p" },
            result: { status: "success", value: {} },
          },
        },
        {
          type: "tool-call-completed",
          callId: "dup-complete",
          modelCallId: "mc",
          toolCall: {
            type: "createPlan",
            args: { plan: "p" },
            result: { status: "success", value: {} },
          },
        },
        {
          type: "partial-tool-call",
          callId: "partial-1",
          modelCallId: "mc",
          toolCall: { type: "shell", args: { command: "ls" } },
        },
        {
          type: "tool-call-delta",
          callId: "task-1",
          modelCallId: "mc-1",
          taskUpdate: { type: "text-delta", text: "nested text" },
        },
        {
          type: "tool-call-delta",
          callId: "task-2",
          modelCallId: "mc-2",
          taskUpdate: {
            type: "tool-call-completed",
            callId: "nested-1",
            modelCallId: "mc",
            toolCall: {
              type: "shell",
              args: { command: "ls" },
              result: {
                status: "success",
                value: {
                  exitCode: 0,
                  signal: "",
                  stdout: "nested out",
                  stderr: "",
                  executionTime: 1,
                },
              },
            },
          },
        },
        {
          type: "tool-call-delta",
          callId: "task-3",
          modelCallId: "mc-3",
          taskUpdate: {
            type: "tool-call-started",
            callId: "nested-2",
            modelCallId: "mc",
            toolCall: { type: "edit", args: { path: "z" } },
          },
        },
        { type: "shell-output-delta", event: { other: 123 } },
        { type: "token-delta", tokens: 5 },
        { type: "turn-ended" },
        {
          type: "turn-ended",
          usage: { inputTokens: 10, outputTokens: 5, cacheReadTokens: 0, cacheWriteTokens: 0 },
        },
      ];
      for (const update of updates) await options?.onDelta?.({ update });
      return result("done");
    },
  });
  const { streamCursor } = await import("../src/provider.ts");

  const events = await collect(streamCursor(MODEL, baseContext()));

  const done = events.at(-1);
  expect(done?.type).toBe("done");
  const content = done?.type === "done" ? done.message.content : [];
  expect(content.length).toBeGreaterThan(0);
  const allText = content
    .filter((item): item is { type: "text"; text: string } => item.type === "text")
    .map((item) => item.text)
    .join("");
  expect(allText).toMatch("shell chunk");
  expect(allText).toMatch("hello");
});

test("formats diffs with or without a header", async () => {
  const { cursorProviderTestApi } = await import("../src/provider.ts");

  const withoutHeader = cursorProviderTestApi.formatDiff("x", "+added");
  expect(withoutHeader).toMatch("--- x");
  expect(withoutHeader).toMatch("+++ x");

  const withHeader = cursorProviderTestApi.formatDiff("y", "--- a/y\n+++ b/y\n-removed");
  expect(withHeader).toMatch("--- a/y");
  expect(withHeader).not.toMatch("--- y\n+++ y");
});

test("extracts shell output text from known keys", async () => {
  const { cursorProviderTestApi } = await import("../src/provider.ts");

  expect(cursorProviderTestApi.shellOutputText({ text: "chunk" })).toBe("chunk");
  expect(cursorProviderTestApi.shellOutputText({ data: "data" })).toBe("data");
  expect(cursorProviderTestApi.shellOutputText({ stdout: "out" })).toBe("out");
  expect(cursorProviderTestApi.shellOutputText({ stderr: "err" })).toBe("err");
  expect(cursorProviderTestApi.shellOutputText({ output: "output" })).toBe("output");
  expect(cursorProviderTestApi.shellOutputText({ unknown: 123 })).toBeUndefined();
});

test("formats started and completed tool calls", async () => {
  const { cursorProviderTestApi } = await import("../src/provider.ts");
  const shell: ToolCall = { type: "shell", args: { command: "ls" } };

  const started = cursorProviderTestApi.formatCursorToolCallUpdate(shell, "started") ?? "";
  expect(started).toMatch("**shell** (started)");
  expect(started).not.toMatch("Exit code");

  const completed =
    cursorProviderTestApi.formatCursorToolCallUpdate(
      {
        type: "shell",
        args: { command: "ls" },
        result: {
          status: "success",
          value: { exitCode: 0, signal: "", stdout: "out", stderr: "err", executionTime: 1 },
        },
      },
      "completed",
    ) ?? "";
  expect(completed).toMatch("**shell** (completed)");
  expect(completed).toMatch("Exit code: 0");
  expect(completed).toMatch("out");
  expect(completed).toMatch("err");
});

test("formats edit tool call results as markdown diffs", async () => {
  const { cursorProviderTestApi } = await import("../src/provider.ts");
  const edit: ToolCall = {
    type: "edit",
    args: { path: "x" },
    result: { status: "success", value: { diffString: "+added" } },
  };

  const text = cursorProviderTestApi.formatCursorToolCallUpdate(edit, "completed") ?? "";
  expect(text).toMatch("**edit** (completed)");
  expect(text).toMatch("```diff");
  expect(text).toMatch("--- x");
  expect(text).toMatch("+added");
});

test("formats remaining tool call result types", async () => {
  const { cursorProviderTestApi } = await import("../src/provider.ts");

  const read: ToolCall = {
    type: "read",
    args: { path: "r" },
    result: { status: "success", value: { content: "file", totalLines: 1, fileSize: 4 } },
  };
  expect(cursorProviderTestApi.formatCursorToolCallUpdate(read, "completed") ?? "").toMatch("file");

  const write: ToolCall = {
    type: "write",
    args: { path: "w", fileText: "new" },
    result: {
      status: "success",
      value: { path: "w", linesCreated: 1, fileSize: 3, fileContentAfterWrite: "new" },
    },
  };
  expect(cursorProviderTestApi.formatCursorToolCallUpdate(write, "completed") ?? "").toMatch("new");

  const writeNoAfter: ToolCall = {
    type: "write",
    args: { path: "w", fileText: "new" },
    result: { status: "success", value: { path: "w", linesCreated: 1, fileSize: 3 } },
  };
  expect(cursorProviderTestApi.formatCursorToolCallUpdate(writeNoAfter, "completed") ?? "").toMatch(
    "write",
  );

  const deleteCall: ToolCall = {
    type: "delete",
    args: { path: "d" },
    result: { status: "success", value: { fileSize: 5 } },
  };
  expect(cursorProviderTestApi.formatCursorToolCallUpdate(deleteCall, "completed") ?? "").toMatch(
    "5",
  );

  const glob: ToolCall = {
    type: "glob",
    args: { globPattern: "*.ts" },
    result: {
      status: "success",
      value: {
        files: ["a.ts", "b.ts"],
        totalFiles: 2,
        clientTruncated: true,
        ripgrepTruncated: false,
      },
    },
  };
  expect(cursorProviderTestApi.formatCursorToolCallUpdate(glob, "completed") ?? "").toMatch("a.ts");

  const globTruncated: ToolCall = {
    type: "glob",
    args: { globPattern: "*.ts" },
    result: {
      status: "success",
      value: {
        files: ["a.ts"],
        totalFiles: 1,
        clientTruncated: false,
        ripgrepTruncated: true,
      },
    },
  };
  expect(
    cursorProviderTestApi.formatCursorToolCallUpdate(globTruncated, "completed") ?? "",
  ).toMatch("ripgrep truncated");

  const mcp: ToolCall = {
    type: "mcp",
    args: { providerIdentifier: "p", toolName: "t" },
    result: {
      status: "success",
      value: { content: [{ text: { text: "mcp result" } }], isError: false },
    },
  };
  expect(cursorProviderTestApi.formatCursorToolCallUpdate(mcp, "completed") ?? "").toMatch(
    "mcp result",
  );

  const mcpEmpty: ToolCall = {
    type: "mcp",
    args: { providerIdentifier: "p" },
    result: {
      status: "success",
      value: { content: [{ image: { data: "data" } }], isError: false },
    },
  };
  expect(cursorProviderTestApi.formatCursorToolCallUpdate(mcpEmpty, "completed") ?? "").toMatch(
    "mcp",
  );

  const createPlan: ToolCall = {
    type: "createPlan",
    args: { plan: "do it" },
    result: { status: "success", value: {} },
  };
  expect(cursorProviderTestApi.formatCursorToolCallUpdate(createPlan, "completed") ?? "").toMatch(
    "createPlan",
  );

  const error: ToolCall = {
    type: "shell",
    args: { command: "false" },
    result: { status: "error", error: new Error("failed") },
  };
  expect(cursorProviderTestApi.formatCursorToolCallUpdate(error, "completed") ?? "").toMatch(
    "Error: failed",
  );
});
