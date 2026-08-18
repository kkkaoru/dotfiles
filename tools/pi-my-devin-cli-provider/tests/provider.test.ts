// This file runs with Bun.
import type {
  Api,
  AssistantMessageEvent,
  AssistantMessageEventStream,
  Context,
  Model,
} from "@earendil-works/pi-ai";
import type { SessionUpdate } from "@agentclientprotocol/sdk";
import { beforeEach, expect, test, vi } from "vitest";

interface RuntimeJob {
  continuationPrompt: string;
  cwd: string;
  initialPrompt: string;
  modelId: string;
  onUpdate: (update: SessionUpdate) => void;
  sessionId: string;
  signal: AbortSignal | undefined;
}

const mocks = vi.hoisted(() => ({
  runDevinJob: vi.fn<(job: RuntimeJob) => Promise<void>>(),
  selectPermission: vi.fn(),
  resolveDevinSessionId: vi.fn<(sessionId: string | undefined) => string>(),
  createDevinSessionId: vi.fn<() => string>(),
}));

vi.mock("../src/runtime.ts", () => ({
  runDevinJob: mocks.runDevinJob,
  selectPermission: mocks.selectPermission,
  resolveDevinSessionId: mocks.resolveDevinSessionId,
  createDevinSessionId: mocks.createDevinSessionId,
}));

const MODEL: Model<Api> = {
  id: "swe-1-7",
  name: "SWE-1.7 Max",
  api: "devin-cli-acp",
  provider: "devin",
  baseUrl: "https://app.devin.ai",
  reasoning: true,
  input: ["text", "image"],
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
  contextWindow: 209_600,
  maxTokens: 128_000,
};
const CONTEXT: Context = {
  systemPrompt: "Be exact.",
  messages: [{ role: "user", content: "Say hello", timestamp: 1 }],
};

async function collect(stream: AssistantMessageEventStream): Promise<AssistantMessageEvent[]> {
  const events: AssistantMessageEvent[] = [];
  for await (const event of stream) events.push(event);
  return events;
}

beforeEach(() => {
  mocks.runDevinJob.mockReset();
  mocks.selectPermission.mockReset();
  mocks.resolveDevinSessionId.mockReset();
  mocks.createDevinSessionId.mockReset();
  mocks.resolveDevinSessionId.mockImplementation((sessionId) => sessionId ?? "generated-session");
  mocks.createDevinSessionId.mockReturnValue("generated-session");
  mocks.selectPermission.mockImplementation((request) => {
    const preferred = request.options[0];
    return preferred
      ? { outcome: { outcome: "selected", optionId: preferred.optionId } }
      : { outcome: { outcome: "cancelled" } };
  });
});

test("re-exports permission selection from the runtime helper", async () => {
  const { selectPermission } = await import("../src/provider.ts");
  selectPermission({
    sessionId: "session",
    toolCall: { toolCallId: "tool" },
    options: [{ optionId: "once", name: "Once", kind: "allow_once" }],
  });
  expect(mocks.selectPermission).toHaveBeenCalledTimes(1);
});

test("streams message and thought updates from a reused ACP job", async () => {
  mocks.runDevinJob.mockImplementation(async (job) => {
    job.onUpdate({
      sessionUpdate: "agent_thought_chunk",
      content: { type: "text", text: "think" },
    });
    job.onUpdate({
      sessionUpdate: "tool_call",
      toolCallId: "tool",
      title: "Read",
      status: "in_progress",
    });
    job.onUpdate({
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "hello" },
    });
  });
  const { streamDevin } = await import("../src/provider.ts");
  const events: AssistantMessageEvent[] = await collect(
    streamDevin(MODEL, CONTEXT, { sessionId: "pi-session-1" }),
  );

  expect(events.map((event) => event.type)).toStrictEqual([
    "start",
    "thinking_start",
    "thinking_delta",
    "thinking_end",
    "text_start",
    "text_delta",
    "done",
  ]);
  expect(mocks.resolveDevinSessionId).toHaveBeenCalledWith("pi-session-1");
  expect(mocks.runDevinJob).toHaveBeenCalledTimes(1);
  expect(mocks.runDevinJob.mock.calls[0]?.[0]?.modelId).toBe("swe-1-7");
  expect(mocks.runDevinJob.mock.calls[0]?.[0]?.sessionId).toBe("pi-session-1");
  expect(mocks.runDevinJob.mock.calls[0]?.[0]?.initialPrompt).toBe(
    "SYSTEM INSTRUCTIONS:\nBe exact.\n\nUSER:\nSay hello\n\nContinue from the transcript above. Follow the latest user request.",
  );
  expect(mocks.runDevinJob.mock.calls[0]?.[0]?.continuationPrompt).toBe("Say hello");
});

test("renders a completed tool_call diff as a text block", async () => {
  mocks.runDevinJob.mockImplementation(async (job) => {
    job.onUpdate({
      sessionUpdate: "tool_call",
      toolCallId: "edit-1",
      title: "Edit x",
      kind: "edit",
      status: "completed",
      content: [{ type: "diff", path: "x", oldText: "a", newText: "b" }],
    });
  });
  const { streamDevin } = await import("../src/provider.ts");
  const events: AssistantMessageEvent[] = await collect(streamDevin(MODEL, CONTEXT));

  expect(events.map((event) => event.type)).toStrictEqual([
    "start",
    "text_start",
    "text_delta",
    "done",
  ]);
  const done = events.at(-1);
  expect(done?.type === "done" ? done.message.content : []).toStrictEqual([
    { type: "text", text: "**Edit x** (edit)\n\n```diff\n--- x\n+++ x\n-a\n+b\n```\n\n" },
  ]);
});

test("renders a completed tool_call_update diff after an in_progress start", async () => {
  mocks.runDevinJob.mockImplementation(async (job) => {
    job.onUpdate({
      sessionUpdate: "tool_call",
      toolCallId: "edit-2",
      title: "Edit y",
      kind: "edit",
      status: "in_progress",
    });
    job.onUpdate({
      sessionUpdate: "tool_call_update",
      toolCallId: "edit-2",
      title: "Edit y",
      kind: "edit",
      status: "completed",
      content: [{ type: "diff", path: "y", oldText: "c", newText: "d" }],
    });
  });
  const { streamDevin } = await import("../src/provider.ts");
  const events: AssistantMessageEvent[] = await collect(streamDevin(MODEL, CONTEXT));

  expect(events.map((event) => event.type)).toStrictEqual([
    "start",
    "text_start",
    "text_delta",
    "done",
  ]);
  const done = events.at(-1);
  expect(done?.type === "done" ? done.message.content : []).toStrictEqual([
    { type: "text", text: "**Edit y** (edit)\n\n```diff\n--- y\n+++ y\n-c\n+d\n```\n\n" },
  ]);
});

test("renders text content from a completed tool call", async () => {
  mocks.runDevinJob.mockImplementation(async (job) => {
    job.onUpdate({
      sessionUpdate: "tool_call_update",
      toolCallId: "read-1",
      title: "Read README",
      kind: "read",
      status: "completed",
      content: [{ type: "content", content: { type: "text", text: "hello world" } }],
    });
  });
  const { streamDevin } = await import("../src/provider.ts");
  const events: AssistantMessageEvent[] = await collect(streamDevin(MODEL, CONTEXT));

  expect(events.map((event) => event.type)).toStrictEqual([
    "start",
    "text_start",
    "text_delta",
    "done",
  ]);
  const done = events.at(-1);
  expect(done?.type === "done" ? done.message.content : []).toStrictEqual([
    { type: "text", text: "**Read README** (read)\n\nhello world\n\n" },
  ]);
});

test("renders a new file diff when oldText is null", async () => {
  mocks.runDevinJob.mockImplementation(async (job) => {
    job.onUpdate({
      sessionUpdate: "tool_call",
      toolCallId: "write-1",
      title: "Create z",
      kind: "edit",
      status: "completed",
      content: [{ type: "diff", path: "z", oldText: null, newText: "new line" }],
    });
  });
  const { streamDevin } = await import("../src/provider.ts");
  const events: AssistantMessageEvent[] = await collect(streamDevin(MODEL, CONTEXT));

  expect(events.map((event) => event.type)).toStrictEqual([
    "start",
    "text_start",
    "text_delta",
    "done",
  ]);
  const done = events.at(-1);
  expect(done?.type === "done" ? done.message.content : []).toStrictEqual([
    { type: "text", text: "**Create z** (edit)\n\n```diff\n+++ z\n+new line\n```\n\n" },
  ]);
});

test("renders a deleted file diff when newText is empty", async () => {
  mocks.runDevinJob.mockImplementation(async (job) => {
    job.onUpdate({
      sessionUpdate: "tool_call",
      toolCallId: "delete-1",
      title: "Delete w",
      kind: "delete",
      status: "completed",
      content: [{ type: "diff", path: "w", oldText: "remove me", newText: "" }],
    });
  });
  const { streamDevin } = await import("../src/provider.ts");
  const events: AssistantMessageEvent[] = await collect(streamDevin(MODEL, CONTEXT));

  expect(events.map((event) => event.type)).toStrictEqual([
    "start",
    "text_start",
    "text_delta",
    "done",
  ]);
  const done = events.at(-1);
  expect(done?.type === "done" ? done.message.content : []).toStrictEqual([
    { type: "text", text: "**Delete w** (delete)\n\n```diff\n--- w\n+++ w\n-remove me\n```\n\n" },
  ]);
});

test("uses the tool kind as the header when a completed tool call has no title", async () => {
  mocks.runDevinJob.mockImplementation(async (job) => {
    job.onUpdate({
      sessionUpdate: "tool_call_update",
      toolCallId: "other-1",
      kind: "other",
      status: "completed",
      content: [{ type: "content", content: { type: "text", text: "note" } }],
    });
  });
  const { streamDevin } = await import("../src/provider.ts");
  const events: AssistantMessageEvent[] = await collect(streamDevin(MODEL, CONTEXT));

  expect(events.map((event) => event.type)).toStrictEqual([
    "start",
    "text_start",
    "text_delta",
    "done",
  ]);
  const done = events.at(-1);
  expect(done?.type === "done" ? done.message.content : []).toStrictEqual([
    { type: "text", text: "**other**\n\nnote\n\n" },
  ]);
});

test("ignores completed tool calls with only non-text content", async () => {
  mocks.runDevinJob.mockImplementation(async (job) => {
    job.onUpdate({
      sessionUpdate: "tool_call_update",
      toolCallId: "terminal-1",
      title: "Run server",
      kind: "execute",
      status: "completed",
      content: [{ type: "terminal", terminalId: "t1" }],
    });
  });
  const { streamDevin } = await import("../src/provider.ts");
  const events: AssistantMessageEvent[] = await collect(streamDevin(MODEL, CONTEXT));

  expect(events.map((event) => event.type)).toStrictEqual(["start", "done"]);
  const done = events.at(-1);
  expect(done?.type === "done" ? done.message.content : []).toStrictEqual([]);
});

test("reports ACP job failures on the assistant stream", async () => {
  mocks.runDevinJob.mockRejectedValue(new Error("connection closed: authentication required"));
  const { streamDevin } = await import("../src/provider.ts");
  const events: AssistantMessageEvent[] = await collect(streamDevin(MODEL, CONTEXT));

  expect(events.map((event) => event.type)).toStrictEqual(["start", "error"]);
  const error = events.at(-1);
  expect(error?.type === "error" ? error.error.errorMessage : "").toBe(
    "connection closed: authentication required",
  );
});

test("marks aborted jobs on the assistant stream", async () => {
  const controller = new AbortController();
  mocks.runDevinJob.mockImplementation(async (job) => {
    await new Promise<void>((resolve) => setImmediate(resolve));
    controller.abort();
    if (job.signal?.aborted) throw new Error("cancelled");
  });
  const { devinProviderTestApi, streamDevin } = await import("../src/provider.ts");
  const stream = streamDevin(MODEL, CONTEXT, { signal: controller.signal });
  const events: AssistantMessageEvent[] = await collect(stream);

  expect(events.map((event) => event.type)).toStrictEqual(["start", "error"]);
  const error = events.at(-1);
  expect(error?.type === "error" ? error.reason : "").toBe("aborted");
  expect(devinProviderTestApi.activeCount()).toBe(0);
  await expect(devinProviderTestApi.waitForIdle()).resolves.toBeUndefined();
});
