// This file runs with bun.
import type {
  Api,
  AssistantMessageEvent,
  Model,
  Tool,
  TranscriptContext,
} from "@earendil-works/pi-ai";
import { expect, it } from "vitest";

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
        role: "user",
        content: [
          { type: "text", text: "see" },
          { type: "image", data: "YWJj", mimeType: "image/jpeg" },
        ],
        timestamp: 1,
      },
      {
        role: "assistant",
        content: [
          { type: "text", text: "checking" },
          {
            type: "toolCall",
            id: "c1",
            name: "bash",
            arguments: { command: "pwd" },
            thoughtSignature: "sig-1",
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
        content: [{ type: "text", text: "boom" }],
        isError: true,
        timestamp: 4,
      },
    ] as TranscriptContext["messages"],
    tools: [READ_TOOL],
    maxTokens: 1024,
    modelMaxTokens: 128000,
  });

  const declaration = body.request.tools?.[0]?.functionDeclarations[0] as Record<string, unknown>;
  expect(declaration["parametersJsonSchema"]).toBeDefined();
  expect(body.request.generationConfig?.["maxOutputTokens"]).toBe(1024);
  expect(body.request.labels["used_claude"]).toBe("true");
  expect(body.request.contents[0]?.parts[1]).toStrictEqual({
    inlineData: { mimeType: "image/jpeg", data: "YWJj" },
  });
  expect(body.request.contents[1]).toStrictEqual({
    role: "model",
    parts: [
      { text: "checking" },
      { functionCall: { name: "bash", args: { command: "pwd" } }, thoughtSignature: "sig-1" },
    ],
  });
  const last = body.request.contents.at(-1);
  expect(last?.role).toBe("user");
  expect(last?.parts).toStrictEqual([
    { functionResponse: { name: "bash", response: { output: "ok" } } },
    { functionResponse: { name: "read", response: { error: "boom" } } },
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
  expect(parseChunk(JSON.stringify({ response: {} }))).toStrictEqual({});

  expect(parseAuthValue(JSON.stringify({ token: "t", projectId: "p" }))).toStrictEqual({
    token: "t",
    projectId: "p",
  });
  expect(parseAuthValue("t")).toStrictEqual({ token: "t" });
  expect(() => parseAuthValue("")).toThrow(/login my-antigravity/);
  expect(() => parseAuthValue("{}")).toThrow(/missing an access token/);
});

it("streams thinking, text, and tool calls through pi events", async () => {
  const events: AssistantMessageEvent[] = [];
  const stream = streamAntigravity(TEST_MODEL, contextWith("sys", [READ_TOOL]), {
    apiKey: JSON.stringify({ token: "tok", projectId: "proj" }),
    reasoning: "medium",
    sessionId: "s1",
    send: async () =>
      sseResponse(200, [
        { response: { candidates: [{ content: { parts: [{ thought: true, text: "plan" }] } }] } },
        { response: { candidates: [{ content: { parts: [{ text: "answer" }] } }] } },
        {
          response: {
            candidates: [
              {
                content: {
                  parts: [{ functionCall: { name: "read", args: { path: "a.ts" } } }],
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
                  parts: [{ thoughtSignature: "sig-late" }],
                },
                finishReason: "STOP",
              },
            ],
            usageMetadata: { promptTokenCount: 5, candidatesTokenCount: 3, thoughtsTokenCount: 1 },
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

it("retries 503 across endpoints", async () => {
  const retryEvents: AssistantMessageEvent[] = [];
  const attempts = { count: 0 };
  for await (const event of streamAntigravity(TEST_MODEL, contextWith("sys"), {
    apiKey: "tok",
    send: async () => {
      attempts.count += 1;
      return sseResponse(503, [{ error: "down" }]);
    },
  })) {
    retryEvents.push(event);
  }
  expect(attempts.count).toBe(2);
  expect(retryEvents.at(-1)?.type).toBe("error");
});
