import type { Api, Model } from "@earendil-works/pi-ai";
import { afterEach, describe, expect, it, vi } from "vitest";
import { toPiContext } from "../src/context-converter.ts";
import { parseClientMessage, type StreamRequestMessage } from "../src/protocol.ts";

const TOKEN = "01234567890123456789012345678901";
const MODEL: Model<Api> = {
  provider: "openai-codex",
  id: "gpt-5.6-luna",
  name: "GPT",
  api: "openai-codex-responses",
  baseUrl: "https://example.test",
  reasoning: true,
  input: ["text", "image"],
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
  contextWindow: 1000,
  maxTokens: 100,
};

function gatewayRequest(overrides: Record<string, unknown> = {}): StreamRequestMessage {
  const message = parseClientMessage(
    JSON.stringify({
      version: 1,
      type: "request",
      id: "request-1",
      token: TOKEN,
      origin: "claudex",
      provider: MODEL.provider,
      modelId: MODEL.id,
      system: null,
      messages: [],
      tools: [],
      options: {},
      ...overrides,
    }),
  );
  if (message.type !== "request") {
    throw new Error("fixture did not parse as request");
  }
  return message;
}

afterEach(() => vi.restoreAllMocks());

describe("Anthropic to Pi context conversion", () => {
  it("preserves system blocks, images, thinking, tools, and tool results", () => {
    vi.spyOn(Date, "now").mockReturnValue(1000);
    const context = toPiContext(
      gatewayRequest({
        system: [
          { type: "text", text: "first" },
          { type: "text", text: "second", cache_control: { type: "ephemeral" } },
        ],
        messages: [
          {
            role: "assistant",
            content: [
              { type: "thinking", thinking: "consider", signature: "sig" },
              { type: "tool_use", id: "tool-1", name: "clock", input: { zone: "UTC" } },
            ],
          },
          {
            role: "system",
            content: [{ type: "text", text: "third" }],
          },
          {
            role: "user",
            content: [
              { type: "text", text: "before" },
              {
                type: "image",
                source: { type: "base64", media_type: "image/png", data: "aGVsbG8=" },
              },
              { type: "tool_result", tool_use_id: "tool-1", content: "12:00", is_error: false },
              { type: "text", text: "after" },
            ],
          },
        ],
        tools: [
          {
            name: "clock",
            description: "Read clock",
            input_schema: { type: "object", properties: { zone: { type: "string" } } },
          },
        ],
      }),
      MODEL,
    );

    expect(context.systemPrompt).toBe("first\n\nsecond\n\nthird");
    expect(context.tools).toStrictEqual([
      {
        name: "clock",
        description: "Read clock",
        parameters: { type: "object", properties: { zone: { type: "string" } } },
      },
    ]);
    expect(context.messages).toStrictEqual([
      {
        role: "assistant",
        content: [
          { type: "thinking", thinking: "consider", thinkingSignature: "sig" },
          { type: "toolCall", id: "tool-1", name: "clock", arguments: { zone: "UTC" } },
        ],
        api: MODEL.api,
        provider: MODEL.provider,
        model: MODEL.id,
        usage: {
          input: 0,
          output: 0,
          cacheRead: 0,
          cacheWrite: 0,
          totalTokens: 0,
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
        },
        stopReason: "toolUse",
        timestamp: 1000,
      },
      {
        role: "user",
        content: [
          { type: "text", text: "before" },
          { type: "image", mimeType: "image/png", data: "aGVsbG8=" },
        ],
        timestamp: 1002,
      },
      {
        role: "toolResult",
        toolCallId: "tool-1",
        toolName: "clock",
        content: [{ type: "text", text: "12:00" }],
        isError: false,
        timestamp: 1000,
      },
      { role: "user", content: [{ type: "text", text: "after" }], timestamp: 1002 },
    ]);
  });

  it("preserves production-scale prompt structure without real session data", () => {
    vi.spyOn(Date, "now").mockReturnValue(1500);
    const tools = Array.from({ length: 61 }, (_unused, index) => ({
      name: `tool-${index}`,
      description: `Synthetic tool ${index}`,
      input_schema: {
        type: "object",
        properties: {
          request: {
            type: "object",
            properties: {
              values: { type: "array", items: { type: "integer" } },
            },
            required: ["values"],
          },
        },
        required: ["request"],
        additionalProperties: false,
      },
    }));
    const toolInput = { request: { values: [1, 2, 3] } };
    const context = toPiContext(
      gatewayRequest({
        system: [
          { type: "text", text: "top-level first" },
          { type: "text", text: "top-level second" },
        ],
        messages: [
          { role: "user", content: "initial request" },
          {
            role: "system",
            content: [
              { type: "text", text: "message-level first" },
              { type: "text", text: "message-level second" },
            ],
          },
          {
            role: "assistant",
            content: [{ type: "tool_use", id: "call-37", name: "tool-37", input: toolInput }],
          },
          {
            role: "user",
            content: [
              {
                type: "tool_result",
                tool_use_id: "call-37",
                content: [{ type: "text", text: "synthetic result" }],
                is_error: true,
              },
            ],
          },
        ],
        tools,
      }),
      MODEL,
    );

    expect(context.systemPrompt).toBe(
      "top-level first\n\ntop-level second\n\nmessage-level first\n\nmessage-level second",
    );
    expect(context.tools).toHaveLength(61);
    expect(context.tools).toStrictEqual(
      tools.map((tool) => ({
        name: tool.name,
        description: tool.description,
        parameters: tool.input_schema,
      })),
    );
    expect(context.messages.map((message) => message.role)).toStrictEqual([
      "user",
      "assistant",
      "toolResult",
    ]);
    expect(context.messages[1]?.content).toStrictEqual([
      { type: "toolCall", id: "call-37", name: "tool-37", arguments: toolInput },
    ]);
    expect(context.messages[2]).toMatchObject({
      role: "toolResult",
      toolCallId: "call-37",
      toolName: "tool-37",
      content: [{ type: "text", text: "synthetic result" }],
      isError: true,
    });
  });

  it("supports string content, redacted thinking, image tool results, and empty descriptions", () => {
    vi.spyOn(Date, "now").mockReturnValue(2000);
    const context = toPiContext(
      gatewayRequest({
        system: "system",
        messages: [
          { role: "assistant", content: [{ type: "redacted_thinking", data: "opaque" }] },
          { role: "assistant", content: "answer" },
          {
            role: "assistant",
            content: [{ type: "tool_use", id: "image-call", name: "vision", input: {} }],
          },
          {
            role: "user",
            content: [
              {
                type: "tool_result",
                tool_use_id: "image-call",
                is_error: true,
                content: [
                  { type: "text", text: "failed" },
                  {
                    type: "image",
                    source: { type: "base64", media_type: "image/jpeg", data: "AA==" },
                  },
                ],
              },
            ],
          },
          { role: "user", content: "next" },
        ],
        tools: [{ name: "vision", input_schema: { type: "object" } }],
      }),
      MODEL,
    );
    expect(context.messages[0]?.content).toStrictEqual([
      { type: "thinking", thinking: "", thinkingSignature: "opaque", redacted: true },
    ]);
    expect(context.messages[1]?.content).toStrictEqual([{ type: "text", text: "answer" }]);
    expect(context.messages[3]).toMatchObject({
      role: "toolResult",
      isError: true,
      content: [
        { type: "text", text: "failed" },
        { type: "image", mimeType: "image/jpeg", data: "AA==" },
      ],
    });
    expect(context.messages[4]).toStrictEqual({ role: "user", content: "next", timestamp: 2004 });
    expect(context.tools?.[0]?.description).toBe("");
  });

  it("rejects unsupported system, message, content, image, and result shapes", () => {
    expect(() => toPiContext(gatewayRequest({ system: {} }), MODEL)).toThrow("system must be");
    expect(() => toPiContext(gatewayRequest({ system: [{ type: "image" }] }), MODEL)).toThrow(
      "system block 0",
    );
    expect(() => toPiContext(gatewayRequest({ messages: [{ role: "developer" }] }), MODEL)).toThrow(
      "unsupported role",
    );
    expect(() =>
      toPiContext(gatewayRequest({ messages: [{ role: "user", content: {} }] }), MODEL),
    ).toThrow("user message 0 content");
    expect(() =>
      toPiContext(
        gatewayRequest({ messages: [{ role: "user", content: [{ type: "document" }] }] }),
        MODEL,
      ),
    ).toThrow("unsupported type");
    expect(() =>
      toPiContext(
        gatewayRequest({
          messages: [
            {
              role: "user",
              content: [{ type: "image", source: { type: "url", url: "https://example.test" } }],
            },
          ],
        }),
        MODEL,
      ),
    ).toThrow("Only base64");
    expect(() =>
      toPiContext(
        gatewayRequest({
          messages: [{ role: "user", content: [{ type: "tool_result", tool_use_id: "missing" }] }],
        }),
        MODEL,
      ),
    ).toThrow("unknown tool call");
  });

  it("rejects malformed assistant blocks, tool result blocks, and tool definitions", () => {
    expect(toPiContext(gatewayRequest(), MODEL)).toStrictEqual({ messages: [] });
    expect(() =>
      toPiContext(
        gatewayRequest({ messages: [{ role: "assistant", content: [{ type: "text" }] }] }),
        MODEL,
      ),
    ).toThrow("field text");
    expect(() =>
      toPiContext(
        gatewayRequest({
          messages: [{ role: "assistant", content: [{ type: "thinking", thinking: "x" }] }],
        }),
        MODEL,
      ),
    ).not.toThrow();
    expect(() =>
      toPiContext(
        gatewayRequest({
          messages: [
            { role: "assistant", content: [{ type: "thinking", thinking: "x", signature: 1 }] },
          ],
        }),
        MODEL,
      ),
    ).toThrow("field signature");
    expect(() =>
      toPiContext(
        gatewayRequest({ messages: [{ role: "assistant", content: [{ type: "citation" }] }] }),
        MODEL,
      ),
    ).toThrow("unsupported type");
    expect(() =>
      toPiContext(gatewayRequest({ messages: [{ role: "assistant", content: {} }] }), MODEL),
    ).toThrow("assistant message 0 content");
    expect(() =>
      toPiContext(
        gatewayRequest({
          messages: [
            { role: "assistant", content: [{ type: "tool_use", id: "t", name: "x", input: {} }] },
            {
              role: "user",
              content: [{ type: "tool_result", tool_use_id: "t", content: [{ type: "json" }] }],
            },
          ],
        }),
        MODEL,
      ),
    ).toThrow("tool result block 0");
    expect(() =>
      toPiContext(
        gatewayRequest({
          messages: [
            { role: "assistant", content: [{ type: "tool_use", id: "t", name: "x", input: {} }] },
            { role: "user", content: [{ type: "tool_result", tool_use_id: "t", content: {} }] },
          ],
        }),
        MODEL,
      ),
    ).toThrow("tool result content");
    expect(() => toPiContext(gatewayRequest({ tools: [{}] }), MODEL)).toThrow("input_schema");
  });
});
