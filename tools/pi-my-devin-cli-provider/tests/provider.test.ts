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
    "text_start",
    "text_delta",
    "done",
  ]);
  const done = events.at(-1);
  expect(done?.type === "done" ? done.message.content : []).toStrictEqual([
    { type: "text", text: "✓ **Edit x** (edit) — completed" },
    {
      type: "text",
      text: "✓ **Edit x** (edit) — completed\n\n**Tool output:**\n```diff\n--- x\n+++ x\n-a\n+b\n```",
    },
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
    "text_start",
    "text_delta",
    "done",
  ]);
  const done = events.at(-1);
  expect(done?.type === "done" ? done.message.content : []).toStrictEqual([
    { type: "text", text: "▶ **Edit y** (edit) — in_progress" },
    {
      type: "text",
      text: "✓ **Edit y** (edit) — completed\n\n**Tool output:**\n```diff\n--- y\n+++ y\n-c\n+d\n```",
    },
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
    "text_start",
    "text_delta",
    "done",
  ]);
  const done = events.at(-1);
  expect(done?.type === "done" ? done.message.content : []).toStrictEqual([
    { type: "text", text: "✓ **Read README** (read) — completed" },
    {
      type: "text",
      text: "✓ **Read README** (read) — completed\n\n**Tool output:**\nhello world",
    },
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
    "text_start",
    "text_delta",
    "done",
  ]);
  const done = events.at(-1);
  expect(done?.type === "done" ? done.message.content : []).toStrictEqual([
    { type: "text", text: "✓ **Create z** (edit) — completed" },
    {
      type: "text",
      text: "✓ **Create z** (edit) — completed\n\n**Tool output:**\n```diff\n+++ z\n+new line\n```",
    },
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
    "text_start",
    "text_delta",
    "done",
  ]);
  const done = events.at(-1);
  expect(done?.type === "done" ? done.message.content : []).toStrictEqual([
    { type: "text", text: "✓ **Delete w** (delete) — completed" },
    {
      type: "text",
      text: "✓ **Delete w** (delete) — completed\n\n**Tool output:**\n```diff\n--- w\n+++ w\n-remove me\n```",
    },
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
    "text_start",
    "text_delta",
    "done",
  ]);
  const done = events.at(-1);
  expect(done?.type === "done" ? done.message.content : []).toStrictEqual([
    { type: "text", text: "✓ **other** (other) — completed" },
    {
      type: "text",
      text: "✓ **other** (other) — completed\n\n**Tool output:**\nnote",
    },
  ]);
});

test("renders completed tool calls with terminal content", async () => {
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

  expect(events.map((event) => event.type)).toStrictEqual([
    "start",
    "text_start",
    "text_delta",
    "text_start",
    "text_delta",
    "done",
  ]);
  const done = events.at(-1);
  expect(done?.type === "done" ? done.message.content : []).toStrictEqual([
    { type: "text", text: "✓ **Run server** (execute) — completed" },
    {
      type: "text",
      text: "✓ **Run server** (execute) — completed\n\n**Terminal output (stdout/stderr):**\nTerminal ID: t1",
    },
  ]);
});

test("renders Devin command snapshots, output, and terminal exit metadata", async () => {
  mocks.runDevinJob.mockImplementation(async (job) => {
    job.onUpdate({
      sessionUpdate: "tool_call",
      toolCallId: "functions.exec:0",
      title: "Ran command",
      kind: "execute",
      content: [
        {
          type: "content",
          content: {
            type: "resource",
            resource: {
              mimeType: "text/x-shellscript",
              text: "printf 'OUT\\n'",
              uri: "tool://preview",
            },
            _meta: { "cognition.ai/preview_is_shell_command": true },
          },
        },
      ],
      rawInput: { command: "printf 'OUT\\n'", workdir: "/tmp/workspace" },
      _meta: { "cognition.ai/inferenceToolName": "exec" },
    });
    job.onUpdate({
      sessionUpdate: "tool_call_update",
      toolCallId: "functions.exec:0",
      status: "in_progress",
      _meta: { "cognition.ai/cwd": "/tmp/workspace" },
    });
    job.onUpdate({
      sessionUpdate: "tool_call_update",
      toolCallId: "functions.exec:0",
      status: "in_progress",
      content: [{ type: "content", content: { type: "text", text: "OUT" } }],
    });
    job.onUpdate({
      sessionUpdate: "tool_call_update",
      toolCallId: "functions.exec:0",
      status: "in_progress",
      content: [{ type: "content", content: { type: "text", text: "OUT\nERR" } }],
      _meta: {
        terminal_exit: { terminal_id: "terminal-1", exit_code: 0, signal: null },
      },
    });
    job.onUpdate({
      sessionUpdate: "tool_call_update",
      toolCallId: "functions.exec:0",
      status: "completed",
      _meta: { "cognition.ai/inferenceToolName": "exec" },
    });
  });
  const { streamDevin } = await import("../src/provider.ts");
  const events: AssistantMessageEvent[] = await collect(streamDevin(MODEL, CONTEXT));

  expect(events.map((event) => event.type)).toStrictEqual([
    "start",
    "text_start",
    "text_delta",
    "text_start",
    "text_delta",
    "done",
  ]);
  const done = events.at(-1);
  expect(done?.type === "done" ? done.message.content : []).toStrictEqual([
    {
      type: "text",
      text: "▶ **Ran command** (execute) — started\n\n**Command:**\n```sh\nprintf 'OUT\\n'\n```\n\n**Working directory:** `/tmp/workspace`",
    },
    {
      type: "text",
      text: "✓ **Ran command** (execute) — completed\n\n**Command:**\n```sh\nprintf 'OUT\\n'\n```\n\n**Working directory:** `/tmp/workspace`\n\n**Terminal output (stdout/stderr):**\nOUT\nERR\n\n**Exit code:** 0\n**Terminal:** `terminal-1`",
    },
  ]);
});

test("renders failed command output and exit status", async () => {
  mocks.runDevinJob.mockImplementation(async (job) => {
    job.onUpdate({
      sessionUpdate: "tool_call",
      toolCallId: "failed-command",
      title: "Run failing command",
      kind: "execute",
      status: "pending",
      rawInput: { command: "false" },
    });
    job.onUpdate({
      sessionUpdate: "tool_call_update",
      toolCallId: "failed-command",
      status: "failed",
      rawOutput: { stdout: "OUT", stderr: "ERR", exitCode: 7 },
    });
  });
  const { streamDevin } = await import("../src/provider.ts");
  const events: AssistantMessageEvent[] = await collect(streamDevin(MODEL, CONTEXT));

  const done = events.at(-1);
  expect(done?.type === "done" ? done.message.content : []).toStrictEqual([
    {
      type: "text",
      text: "▶ **Run failing command** (execute) — pending\n\n**Command:**\n```sh\nfalse\n```",
    },
    {
      type: "text",
      text: "✗ **Run failing command** (execute) — failed\n\n**Command:**\n```sh\nfalse\n```\n\n**stdout:**\n```\nOUT\n```\n\n**stderr:**\n```\nERR\n```\n\n**Exit code:** 7",
    },
  ]);
});

test("retains Devin edit diffs across status-only updates", async () => {
  mocks.runDevinJob.mockImplementation(async (job) => {
    job.onUpdate({
      sessionUpdate: "tool_call",
      toolCallId: "functions.write:0",
      title: "Wrote probe.txt",
      kind: "edit",
      content: [{ type: "diff", path: "/tmp/probe.txt", newText: "PROBE\n" }],
      rawInput: { file_path: "/tmp/probe.txt", content: "PROBE\n" },
    });
    job.onUpdate({
      sessionUpdate: "tool_call_update",
      toolCallId: "functions.write:0",
      status: "in_progress",
    });
    job.onUpdate({
      sessionUpdate: "tool_call_update",
      toolCallId: "functions.write:0",
      status: "completed",
    });
  });
  const { streamDevin } = await import("../src/provider.ts");
  const events: AssistantMessageEvent[] = await collect(streamDevin(MODEL, CONTEXT));

  const done = events.at(-1);
  expect(done?.type === "done" ? done.message.content : []).toStrictEqual([
    {
      type: "text",
      text: "▶ **Wrote probe.txt** (edit) — started\n\n**Path:** `/tmp/probe.txt`",
    },
    {
      type: "text",
      text: "✓ **Wrote probe.txt** (edit) — completed\n\n**Path:** `/tmp/probe.txt`\n\n**Tool output:**\n```diff\n+++ /tmp/probe.txt\n+PROBE\n```",
    },
  ]);
});

test("formats ACP resource and media content with a raw input fallback", async () => {
  mocks.runDevinJob.mockImplementation(async (job) => {
    job.onUpdate({
      sessionUpdate: "tool_call_update",
      toolCallId: "inspect-1",
      title: "Inspect resources",
      kind: "other",
      status: "completed",
      rawInput: { query: "probe" },
      rawOutput: { output: "raw output" },
      content: [
        {
          type: "content",
          content: {
            type: "resource",
            resource: { uri: "resource://text", text: "resource text" },
          },
        },
        {
          type: "content",
          content: { type: "resource_link", name: "linked", uri: "resource://linked" },
        },
        {
          type: "content",
          content: { type: "image", data: "image", mimeType: "image/png" },
        },
        {
          type: "content",
          content: { type: "audio", data: "audio", mimeType: "audio/wav" },
        },
        {
          type: "content",
          content: {
            type: "resource",
            resource: {
              uri: "resource://binary",
              blob: "binary",
              mimeType: "application/octet-stream",
            },
          },
        },
      ],
    });
  });
  const { streamDevin } = await import("../src/provider.ts");
  const events: AssistantMessageEvent[] = await collect(streamDevin(MODEL, CONTEXT));

  const done = events.at(-1);
  expect(done?.type === "done" ? done.message.content : []).toStrictEqual([
    {
      type: "text",
      text: '✓ **Inspect resources** (other) — completed\n\n**Input:**\n```json\n{\n  "query": "probe"\n}\n```',
    },
    {
      type: "text",
      text: '✓ **Inspect resources** (other) — completed\n\n**Input:**\n```json\n{\n  "query": "probe"\n}\n```\n\n**Tool output:**\nresource text\nResource: linked (resource://linked)\n[image: image/png]\n[audio: audio/wav]\n[binary resource: application/octet-stream]\n\n**Output:**\n```\nraw output\n```',
    },
  ]);
});

test("renders a non-structured raw output value", async () => {
  mocks.runDevinJob.mockImplementation(async (job) => {
    job.onUpdate({
      sessionUpdate: "tool_call_update",
      toolCallId: "raw-1",
      title: "Return raw output",
      status: "completed",
      rawOutput: "raw output",
    });
  });
  const { streamDevin } = await import("../src/provider.ts");
  const events: AssistantMessageEvent[] = await collect(streamDevin(MODEL, CONTEXT));

  const done = events.at(-1);
  expect(done?.type === "done" ? done.message.content : []).toStrictEqual([
    { type: "text", text: "✓ **Return raw output** (other) — completed" },
    {
      type: "text",
      text: "✓ **Return raw output** (other) — completed\n\n**Raw output:**\n```json\nraw output\n```",
    },
  ]);
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
