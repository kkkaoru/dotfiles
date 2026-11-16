// This file runs with bun.
import type {
  Api,
  AssistantMessageEvent,
  Model,
  Tool,
  TranscriptContext,
} from "@earendil-works/pi-ai";
import { expect, it } from "vitest";

import { rememberModelEnums } from "../src/client.ts";
import { buildGenerateBody, parseAuthValue, parseChunk, streamAntigravity } from "../src/stream.ts";
import type { WireResponse } from "../src/transport.ts";

function contextWith(systemPrompt: string, tools: Tool[] = []): TranscriptContext {
  return {
    messages: [
      {
        role: "system",
        content: systemPrompt,
        timestamp: Date.now(),
        ...(tools.length ? { toolsAdded: tools } : {}),
      },
      { role: "user", content: "こんにちは", timestamp: Date.now() },
    ],
  } as TranscriptContext;
}

const READ_TOOL = {
  name: "read",
  description: "Read a file",
  parameters: { type: "object", properties: { path: { type: "string" } }, required: ["path"] },
} as unknown as Tool;

const TEST_MODEL: Model<Api> = {
  id: "gemini-4-hydrogen",
  name: "Gemini 4 Hydrogen",
  api: "my-antigravity-api",
  provider: "my-antigravity",
  baseUrl: "https://daily-cloudcode-pa.googleapis.com",
  input: ["text", "image"],
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
  reasoning: true,
  contextWindow: 1048576,
  maxTokens: 262144,
};

function sseResponse(status: number, events: unknown[]): WireResponse {
  const payload = events.map((event) => `data: ${JSON.stringify(event)}\n\n`).join("");
  return {
    status,
    headers: {},
    body: (async function* () {
      yield Buffer.from(payload.slice(0, 15));
      yield Buffer.from(payload.slice(15));
    })(),
    text: async () => payload,
  };
}

it("builds request with system prompt as leading user turn and never systemInstruction", () => {
  const body = buildGenerateBody({
    project: "aicode-consumers",
    publicModelId: "gemini-4-hydrogen",
    level: "medium",
    systemPrompt: "You are a coding assistant.",
    messages: contextWith("You are a coding assistant.").messages,
    tools: [],
    maxTokens: undefined,
    modelMaxTokens: 262144,
    sessionId: "sess-1",
    modelEnum: "MODEL_PLACEHOLDER_M526",
  });

  expect(body.model).toBe("gemini-4-hydrogen-medium");
  expect(body.requestType).toBe("agent");
  expect(body.userAgent).toBe("antigravity");
  expect(body.request.contents[0]).toStrictEqual({
    role: "user",
    parts: [{ text: "You are a coding assistant." }],
  });
  expect(body.request.contents[1]).toStrictEqual({ role: "user", parts: [{ text: "こんにちは" }] });
  expect("systemInstruction" in body.request).toBe(false);
  expect(body.request.labels["model_enum"]).toBe("MODEL_PLACEHOLDER_M526");
});

it("declares tools via parametersJsonSchema and replays images, assistant calls, and tool results", () => {
  const body = buildGenerateBody({
    project: "aicode-consumers",
    publicModelId: "claude-sonnet-5-5",
    level: "high",
    systemPrompt: undefined,
    messages: [
      {
        role: "system",
        content: "ignored",
        timestamp: 0,
      },
      {
        role: "user",
        content: [
          { type: "text", text: "see" },
          { type: "image", data: "YWJj", mimeType: "image/jpeg" },
          { type: "image", data: "ZGVm", mimeType: "" },
        ],
        timestamp: 1,
      },
      {
        role: "assistant",
        content: [{ type: "text", text: "failed turn" }],
        api: "my-antigravity-api",
        provider: "my-antigravity",
        model: "claude-sonnet-5-5",
        usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: {} },
        stopReason: "error",
        timestamp: 1,
      },
      {
        role: "assistant",
        content: [
          { type: "thinking", thinking: "hmm", thinkingSignature: "sig-think" },
          { type: "thinking", thinking: "unsigned" },
          { type: "text", text: "   " },
          { type: "text", text: "checking" },
          {
            type: "toolCall",
            id: "c1",
            name: "bash",
            arguments: { command: "pwd" },
            thoughtSignature: "sig-1",
          },
          {
            type: "toolCall",
            id: "c2",
            name: "read",
            arguments: {},
          },
        ],
        api: "my-antigravity-api",
        provider: "my-antigravity",
        model: "claude-sonnet-5-5",
        usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: {} },
        stopReason: "toolUse",
        timestamp: 2,
      },
      {
        role: "toolResult",
        toolCallId: "c1",
        toolName: "bash",
        content: [{ type: "text", text: "ok" }],
        isError: false,
        timestamp: 3,
      },
      {
        role: "toolResult",
        toolCallId: "c2",
        toolName: "read",
        content: [
          { type: "text", text: "boom" },
          { type: "image", data: "img", mimeType: "image/png" },
        ],
        isError: true,
        timestamp: 4,
      },
    ] as TranscriptContext["messages"],
    tools: [READ_TOOL],
    maxTokens: 1024,
    modelMaxTokens: 128000,
  });

  const declaration = body.request.tools?.[0]?.functionDeclarations[0];
  expect(declaration?.parametersJsonSchema).toBeDefined();
  // Second call with the same tools array exercises TOOL_CACHE hit.
  const cachedTools = [READ_TOOL];
  const cachedFirst = buildGenerateBody({
    project: "aicode-consumers",
    publicModelId: "gemini-4-hydrogen",
    level: "low",
    systemPrompt: undefined,
    messages: contextWith("").messages,
    tools: cachedTools,
    maxTokens: undefined,
    modelMaxTokens: 65536,
  });
  const cachedSecond = buildGenerateBody({
    project: "aicode-consumers",
    publicModelId: "gemini-4-hydrogen",
    level: "low",
    systemPrompt: undefined,
    messages: contextWith("").messages,
    tools: cachedTools,
    maxTokens: undefined,
    modelMaxTokens: 65536,
  });
  expect(cachedSecond.request.tools).toBe(cachedFirst.request.tools);

  const otherTool = { ...READ_TOOL, name: "write" } as unknown as Tool;
  const cachedThird = buildGenerateBody({
    project: "aicode-consumers",
    publicModelId: "gemini-4-hydrogen",
    level: "low",
    systemPrompt: undefined,
    messages: contextWith("").messages,
    tools: [otherTool],
    maxTokens: undefined,
    modelMaxTokens: 65536,
  });
  expect(cachedThird.request.tools).not.toBe(cachedFirst.request.tools);
  expect(body.request.generationConfig?.["maxOutputTokens"]).toBe(1024);
  expect(body.request.labels["used_claude"]).toBe("true");
  expect(body.request.contents[0]?.parts[1]).toStrictEqual({
    inlineData: { mimeType: "image/jpeg", data: "YWJj" },
  });
  expect(body.request.contents[0]?.parts[2]).toStrictEqual({
    inlineData: { mimeType: "image/png", data: "ZGVm" },
  });
  expect(body.request.contents[1]).toStrictEqual({
    role: "model",
    parts: [
      { thought: true, text: "hmm", thoughtSignature: "sig-think" },
      { text: "checking" },
      {
        functionCall: { name: "bash", args: { command: "pwd" }, id: "c1" },
        thoughtSignature: "sig-1",
      },
      { functionCall: { name: "read", args: {}, id: "c2" } },
    ],
  });
  const last = body.request.contents.at(-1);
  expect(last?.role).toBe("user");
  expect(last?.parts).toStrictEqual([
    { functionResponse: { name: "bash", response: { output: "ok" }, id: "c1" } },
    { functionResponse: { name: "read", response: { error: "boom\n[image]" }, id: "c2" } },
  ]);

  // Replaying an unsigned Claude/GPT-OSS tool call on Gemini 4 Argon sends `skip_thought_signature_validator` on native `functionCall`.
  const argonReplay = buildGenerateBody({
    project: "aicode-consumers",
    publicModelId: "gemini-4-argon",
    level: "medium",
    systemPrompt: undefined,
    messages: [
      { role: "user", content: "run pwd", timestamp: 1 },
      {
        role: "assistant",
        content: [{ type: "toolCall", id: "u1", name: "bash", arguments: { command: "pwd" } }],
        api: "my-antigravity-api",
        provider: "my-antigravity",
        model: "claude-sonnet-5-5",
        usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: {} },
        stopReason: "toolUse",
        timestamp: 2,
      },
      {
        role: "toolResult",
        toolCallId: "u1",
        toolName: "bash",
        content: [{ type: "text", text: "/tmp" }],
        isError: false,
        timestamp: 3,
      },
    ] as TranscriptContext["messages"],
    tools: [READ_TOOL],
    maxTokens: undefined,
    modelMaxTokens: 65536,
  });
  expect(argonReplay.request.contents[1]).toStrictEqual({
    role: "model",
    parts: [
      {
        functionCall: { name: "bash", args: { command: "pwd" } },
        thoughtSignature: "skip_thought_signature_validator",
      },
    ],
  });
  expect(argonReplay.request.contents[2]).toStrictEqual({
    role: "user",
    parts: [{ functionResponse: { name: "bash", response: { output: "/tmp" } } }],
  });

  // Legacy mimicked "[Tool call ...]" assistant text blocks are omitted from replay.
  const mimickedReplay = buildGenerateBody({
    project: "aicode-consumers",
    publicModelId: "gemini-4-hydrogen",
    level: "medium",
    systemPrompt: undefined,
    messages: [
      { role: "user", content: "first", timestamp: 0 },
      {
        role: "assistant",
        content: [{ type: "text", text: "ok" }],
        api: "my-antigravity-api",
        provider: "my-antigravity",
        model: "gemini-4-hydrogen",
        usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: {} },
        stopReason: "stop",
        timestamp: 1,
      },
      { role: "user", content: "continue", timestamp: 2 },
      {
        role: "assistant",
        content: [
          { type: "text", text: '[Tool call read({"path":"/tmp/a"})]' },
          { type: "text", text: '(called read with {"path":"/tmp/b"})' },
        ],
        api: "my-antigravity-api",
        provider: "my-antigravity",
        model: "gemini-4-hydrogen",
        usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: {} },
        stopReason: "stop",
        timestamp: 3,
      },
      { role: "user", content: "continue", timestamp: 4 },
    ] as TranscriptContext["messages"],
    tools: [READ_TOOL],
    maxTokens: undefined,
    modelMaxTokens: 65536,
  });
  expect(mimickedReplay.request.contents).toStrictEqual([
    { role: "user", parts: [{ text: "first" }] },
    { role: "model", parts: [{ text: "ok" }] },
    { role: "user", parts: [{ text: "continue" }, { text: "continue" }] },
  ]);

  // Consecutive surviving model turns after an omitted user/system turn merge cleanly.
  const mergedModelReplay = buildGenerateBody({
    project: "aicode-consumers",
    publicModelId: "gemini-4-hydrogen",
    level: "medium",
    systemPrompt: undefined,
    messages: [
      { role: "user", content: "hi", timestamp: 0 },
      {
        role: "assistant",
        content: [{ type: "text", text: "step 1" }],
        api: "my-antigravity-api",
        provider: "my-antigravity",
        model: "gemini-4-hydrogen",
        usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: {} },
        stopReason: "stop",
        timestamp: 1,
      },
      {
        role: "assistant",
        content: [{ type: "text", text: "step 2" }],
        api: "my-antigravity-api",
        provider: "my-antigravity",
        model: "gemini-4-hydrogen",
        usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: {} },
        stopReason: "stop",
        timestamp: 2,
      },
    ] as TranscriptContext["messages"],
    tools: [READ_TOOL],
    maxTokens: undefined,
    modelMaxTokens: 65536,
  });
  expect(mergedModelReplay.request.contents).toStrictEqual([
    { role: "user", parts: [{ text: "hi" }] },
    { role: "model", parts: [{ text: "step 1" }, { text: "step 2" }] },
  ]);

  // Merge consecutive user turns when a followUp (e.g. tmux completion batch) arrives after a user message.
  const followUpBatch = buildGenerateBody({
    project: "aicode-consumers",
    publicModelId: "gemini-4-hydrogen",
    level: "medium",
    systemPrompt: undefined,
    messages: [
      { role: "user", content: "start", timestamp: 0 },
      {
        role: "assistant",
        content: [{ type: "text", text: "Started detached tmux command." }],
        api: "my-antigravity-api",
        provider: "my-antigravity",
        model: "gemini-4-hydrogen",
        usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: {} },
        stopReason: "stop",
        timestamp: 1,
      },
      { role: "user", content: "tmux completion batch: 2 tasks completed", timestamp: 2 },
      { role: "user", content: "continue", timestamp: 3 },
    ] as TranscriptContext["messages"],
    tools: [READ_TOOL],
    maxTokens: undefined,
    modelMaxTokens: 65536,
  });
  expect(followUpBatch.request.contents).toStrictEqual([
    { role: "user", parts: [{ text: "start" }] },
    { role: "model", parts: [{ text: "Started detached tmux command." }] },
    {
      role: "user",
      parts: [{ text: "tmux completion batch: 2 tasks completed" }, { text: "continue" }],
    },
  ]);
});

it("parses SSE chunks and auth values", () => {
  expect(
    parseChunk(
      JSON.stringify({
        response: {
          candidates: [{ content: { parts: [{ text: "hello" }] } }],
          usageMetadata: { promptTokenCount: 10, candidatesTokenCount: 2, thoughtsTokenCount: 1 },
          responseId: "abc",
        },
      }),
    ),
  ).toStrictEqual({
    text: "hello",
    usage: { input: 10, output: 2, reasoning: 1 },
    responseId: "abc",
  });

  expect(
    parseChunk(
      JSON.stringify({
        response: { candidates: [{ content: { parts: [{ thought: true, text: "hmm" }] } }] },
      }),
    ),
  ).toStrictEqual({ thought: "hmm" });

  expect(
    parseChunk(
      JSON.stringify({
        response: {
          candidates: [
            {
              content: {
                parts: [
                  {
                    functionCall: { name: "bash", args: { command: "ls" } },
                    thoughtSignature: "sig-x",
                  },
                ],
              },
              finishReason: "STOP",
            },
          ],
        },
      }),
    ),
  ).toStrictEqual({
    signature: "sig-x",
    toolCall: { name: "bash", args: { command: "ls" }, thoughtSignature: "sig-x" },
    finishReason: "STOP",
  });

  expect(parseChunk("not json")).toBeUndefined();
  expect(parseChunk("null")).toBeUndefined();
  expect(parseChunk(JSON.stringify({ response: {} }))).toStrictEqual({});
  expect(parseChunk(JSON.stringify({ candidates: [{}] }))).toStrictEqual({});
  expect(parseChunk(JSON.stringify({ candidates: [] }))).toStrictEqual({});
  expect(
    parseChunk(
      JSON.stringify({
        candidates: [
          { content: { parts: [{ text: 123 }, { functionCall: { name: "x", args: "bad" } }] } },
        ],
      }),
    ),
  ).toStrictEqual({ toolCall: { name: "x", args: {} } });
  expect(
    parseChunk(
      JSON.stringify({
        candidates: [{ content: { parts: [{ text: "a" }] }, finishReason: "OTHER" }],
      }),
    ),
  ).toStrictEqual({ text: "a", finishReason: "OTHER" });

  // Also test streamAntigravity with finishReason "OTHER" (not MAX_TOKENS) and empty stream.
  expect(
    parseChunk(
      JSON.stringify({
        candidates: [
          {
            content: {
              parts: [
                null,
                { functionCall: {} },
                {
                  functionCall: {
                    name: "bash",
                    args: { ok: true, count: 2, empty: null, list: ["x", 1], nested: { a: "b" } },
                  },
                },
                { text: "" },
              ],
            },
          },
        ],
        usageMetadata: {},
      }),
    ),
  ).toStrictEqual({
    toolCall: {
      name: "bash",
      args: { ok: true, count: 2, empty: null, list: ["x", 1], nested: { a: "b" } },
    },
    usage: {},
  });

  expect(parseAuthValue(JSON.stringify({ token: "t", projectId: "p" }))).toStrictEqual({
    token: "t",
    projectId: "p",
  });
  expect(parseAuthValue("t")).toStrictEqual({ token: "t" });
  expect(() => parseAuthValue(undefined)).toThrow(/login my-antigravity/);
  expect(() => parseAuthValue("")).toThrow(/login my-antigravity/);
  expect(() => parseAuthValue("{}")).toThrow(/missing an access token/);
});

it("streams thinking, text, and tool calls through pi events", async () => {
  rememberModelEnums([{ id: "gemini-4-hydrogen-medium", modelEnum: "MODEL_PLACEHOLDER_M526" }]);
  const events: AssistantMessageEvent[] = [];
  const stream = streamAntigravity(TEST_MODEL, contextWith("sys", [READ_TOOL]), {
    apiKey: JSON.stringify({ token: "tok", projectId: "proj" }),
    reasoning: "medium",
    sessionId: "s1",
    send: async () =>
      sseResponse(200, [
        {
          response: {
            candidates: [
              { content: { parts: [{ thought: true, text: "plan", thoughtSignature: "sig-t" }] } },
            ],
          },
        },
        { response: { candidates: [{ content: { parts: [{ thought: true, text: " more" }] } }] } },
        { response: { candidates: [{ content: { parts: [{ text: "answer" }] } }] } },
        { response: { candidates: [{ content: { parts: [{ text: "!" }] } }] } },
        {
          response: {
            candidates: [
              {
                content: {
                  parts: [
                    {
                      functionCall: { id: "call-bash", name: "bash", args: { command: "pwd" } },
                      thoughtSignature: "sig-inline",
                    },
                  ],
                },
              },
            ],
          },
        },
        {
          response: {
            candidates: [
              {
                content: {
                  parts: [{ functionCall: { name: "read", args: { path: "a.ts" } } }],
                },
              },
            ],
            usageMetadata: { promptTokenCount: 5, candidatesTokenCount: 3, thoughtsTokenCount: 1 },
          },
        },
        {
          response: {
            candidates: [
              {
                content: {
                  parts: [{ thoughtSignature: "sig-late" }],
                },
                finishReason: "STOP",
              },
            ],
            usageMetadata: {},
            responseId: "resp-1",
          },
        },
      ]),
  });

  for await (const event of stream) {
    events.push(event);
  }
  const done = events.at(-1);
  expect(done?.type).toBe("done");
  if (done?.type === "done") {
    expect(done.reason).toBe("toolUse");
    expect(done.message.responseId).toBe("resp-1");
    expect(done.message.usage.totalTokens).toBe(8);
  }
});

it("handles MAX_TOKENS during streaming", async () => {
  const lengthEvents: AssistantMessageEvent[] = [];
  const lengthStream = streamAntigravity(TEST_MODEL, contextWith("sys"), {
    apiKey: "tok",
    send: async () =>
      sseResponse(200, [
        {
          response: {
            candidates: [{ content: { parts: [{ text: "cut" }] }, finishReason: "MAX_TOKENS" }],
          },
        },
      ]),
  });
  for await (const event of lengthStream) {
    lengthEvents.push(event);
  }
  const done = lengthEvents.at(-1);
  expect(done?.type).toBe("done");
  if (done?.type === "done") {
    expect(done.reason).toBe("length");
  }
});

it("emits error events for 429, 401, and retries 503 across endpoints", async () => {
  const rateLimitedEvents: AssistantMessageEvent[] = [];
  for await (const event of streamAntigravity(TEST_MODEL, contextWith("sys"), {
    apiKey: "tok",
    send: async () => sseResponse(429, [{ error: "busy" }]),
  })) {
    rateLimitedEvents.push(event);
  }
  const errorEvent = rateLimitedEvents.at(-1);
  expect(errorEvent?.type).toBe("error");
  if (errorEvent?.type === "error") {
    expect(errorEvent.error.errorMessage).toMatch(/429/);
  }

  const authEvents: AssistantMessageEvent[] = [];
  for await (const event of streamAntigravity(TEST_MODEL, contextWith("sys"), {
    apiKey: "tok",
    send: async () => sseResponse(401, [{ error: "bad" }]),
  })) {
    authEvents.push(event);
  }
  const authErr = authEvents.at(-1);
  expect(authErr?.type).toBe("error");
  if (authErr?.type === "error") {
    expect(authErr.error.errorMessage).toMatch(/401/);
  }
});

it("retries 503 across endpoints and handles CRLF + non-Error throws", async () => {
  const retryEvents: AssistantMessageEvent[] = [];
  const attempts = { count: 0 };
  for await (const event of streamAntigravity({ ...TEST_MODEL, maxTokens: 0 }, contextWith("sys"), {
    apiKey: "tok",
    signal: new AbortController().signal,
    send: async () => {
      attempts.count += 1;
      return sseResponse(503, [{ error: "down" }]);
    },
  })) {
    retryEvents.push(event);
  }
  expect(attempts.count).toBe(2);
  expect(retryEvents.at(-1)?.type).toBe("error");

  const nonErrorEvents: AssistantMessageEvent[] = [];
  for await (const event of streamAntigravity(TEST_MODEL, contextWith("sys"), {
    apiKey: "tok",
    send: async () => {
      throw "string-boom";
    },
  })) {
    nonErrorEvents.push(event);
  }
  const strErr = nonErrorEvents.at(-1);
  expect(strErr?.type).toBe("error");
  if (strErr?.type === "error") {
    expect(strErr.error.errorMessage).toBe("string-boom");
  }

  const nonRetryEvents: AssistantMessageEvent[] = [];
  const badReqAttempts = { count: 0 };
  for await (const event of streamAntigravity(TEST_MODEL, contextWith("sys"), {
    apiKey: "tok",
    send: async () => {
      badReqAttempts.count += 1;
      return sseResponse(400, [{ error: "bad request" }]);
    },
  })) {
    nonRetryEvents.push(event);
  }
  expect(badReqAttempts.count).toBe(1);
  expect(nonRetryEvents.at(-1)?.type).toBe("error");
});

it("retries empty streams once and errors if still empty", async () => {
  const emptyEvents: AssistantMessageEvent[] = [];
  const emptyAttempts = { count: 0 };
  for await (const event of streamAntigravity(TEST_MODEL, contextWith(""), {
    apiKey: "tok",
    send: async () => {
      emptyAttempts.count += 1;
      return sseResponse(200, [{ response: {} }]);
    },
  })) {
    emptyEvents.push(event);
  }
  expect(emptyAttempts.count).toBe(2);
  const emptyErr = emptyEvents.at(-1);
  expect(emptyErr?.type).toBe("error");
  if (emptyErr?.type === "error") {
    expect(emptyErr.error.errorMessage).toMatch(/empty response/);
  }

  // Transient empty response followed by a normal response succeeds on retry.
  const recoveredEvents: AssistantMessageEvent[] = [];
  const recoverAttempts = { count: 0 };
  for await (const event of streamAntigravity(TEST_MODEL, contextWith(""), {
    apiKey: "tok",
    send: async () => {
      recoverAttempts.count += 1;
      return recoverAttempts.count === 1
        ? sseResponse(200, [{ response: {} }])
        : sseResponse(200, [
            {
              response: {
                candidates: [{ content: { parts: [{ text: "ok" }] }, finishReason: "STOP" }],
              },
            },
          ]);
    },
  })) {
    recoveredEvents.push(event);
  }
  expect(recoverAttempts.count).toBe(2);
  expect(recoveredEvents.at(-1)?.type).toBe("done");
});

it("handles CRLF framing and EOF without trailing newline", async () => {
  const crlfEvents: AssistantMessageEvent[] = [];
  const crlfPayload = `data: {"response":{"candidates":[{"content":{"parts":\r\ndata: [{"text":"crlf"}]},"finishReason":"STOP"}]}}\r\n\r\n`;
  for await (const event of streamAntigravity(TEST_MODEL, contextWith("sys"), {
    apiKey: "tok",
    send: async () => ({
      status: 200,
      headers: {},
      body: (async function* () {
        yield Buffer.from(crlfPayload);
      })(),
      text: async () => crlfPayload,
    }),
  })) {
    crlfEvents.push(event);
  }
  expect(crlfEvents.at(-1)?.type).toBe("done");

  // Stream ending without a trailing newline flushes the final data line on EOF.
  const eofEvents: AssistantMessageEvent[] = [];
  const eofPayload = `data: ${JSON.stringify({ response: { candidates: [{ content: { parts: [{ text: "eof" }] }, finishReason: "STOP" }] } })}`;
  for await (const event of streamAntigravity(TEST_MODEL, contextWith("sys"), {
    apiKey: "tok",
    send: async () => ({
      status: 200,
      headers: {},
      body: (async function* () {
        yield Buffer.from(eofPayload);
      })(),
      text: async () => eofPayload,
    }),
  })) {
    eofEvents.push(event);
  }
  const eofDone = eofEvents.at(-1);
  expect(eofDone?.type).toBe("done");
  if (eofDone?.type === "done") {
    expect(eofDone.message.content[0]).toStrictEqual({ type: "text", text: "eof" });
  }
});

it("streams large SSE payloads in linear time", async () => {
  const chunkLine = `data: ${JSON.stringify({ response: { candidates: [{ content: { parts: [{ text: "x" }] } }] } })}\n\n`;
  const runBatch = async (count: number): Promise<number> => {
    const payload = chunkLine.repeat(count);
    const start = performance.now();
    for await (const _event of streamAntigravity(TEST_MODEL, contextWith("sys"), {
      apiKey: "tok",
      send: async () => ({
        status: 200,
        headers: {},
        body: (async function* () {
          yield Buffer.from(payload);
        })(),
        text: async () => payload,
      }),
    })) {
      // Drain stream events to measure total parsing and emission scaling.
    }
    return performance.now() - start;
  };

  await runBatch(500);
  const smallMs = Math.max(await runBatch(1000), 1);
  const largeMs = await runBatch(10000);
  expect(largeMs / smallMs).toBeLessThan(25);
});
