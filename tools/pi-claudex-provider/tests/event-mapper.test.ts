import type { AssistantMessage, AssistantMessageEvent } from "@earendil-works/pi-ai";
import { describe, expect, it } from "vitest";
import { mapAssistantEvent } from "../src/event-mapper.ts";

const MESSAGE: AssistantMessage = {
  role: "assistant",
  content: [],
  api: "openai-completions",
  provider: "ollama-cloud",
  model: "glm-5.2",
  usage: {
    input: 1,
    output: 2,
    cacheRead: 0,
    cacheWrite: 0,
    totalTokens: 3,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
  },
  stopReason: "pending",
  timestamp: 1,
};

function map(event: AssistantMessageEvent) {
  return mapAssistantEvent("request", event);
}

describe("Pi assistant event mapping", () => {
  it("maps start and text lifecycle events", () => {
    expect(map({ type: "start", partial: MESSAGE })).toStrictEqual({
      version: 1,
      type: "start",
      id: "request",
      provider: "ollama-cloud",
      model: "glm-5.2",
      api: "openai-completions",
    });
    expect(map({ type: "text_start", contentIndex: 0, partial: MESSAGE })).toStrictEqual({
      version: 1,
      type: "text_start",
      id: "request",
      index: 0,
    });
    expect(
      map({ type: "text_delta", contentIndex: 0, delta: "hi", partial: MESSAGE }),
    ).toStrictEqual({
      version: 1,
      type: "text_delta",
      id: "request",
      index: 0,
      delta: "hi",
    });
    expect(
      map({ type: "text_end", contentIndex: 0, content: "hi", partial: MESSAGE }),
    ).toStrictEqual({
      version: 1,
      type: "text_end",
      id: "request",
      index: 0,
      content: "hi",
    });
  });

  it("maps thinking lifecycle events", () => {
    expect(map({ type: "thinking_start", contentIndex: 1, partial: MESSAGE })).toStrictEqual({
      version: 1,
      type: "thinking_start",
      id: "request",
      index: 1,
    });
    expect(
      map({ type: "thinking_delta", contentIndex: 1, delta: "why", partial: MESSAGE }),
    ).toStrictEqual({
      version: 1,
      type: "thinking_delta",
      id: "request",
      index: 1,
      delta: "why",
    });
    expect(
      map({ type: "thinking_end", contentIndex: 1, content: "why", partial: MESSAGE }),
    ).toStrictEqual({
      version: 1,
      type: "thinking_end",
      id: "request",
      index: 1,
      content: "why",
    });
  });

  it("maps tool-call lifecycle events", () => {
    const partial: AssistantMessage = {
      ...MESSAGE,
      content: [{ type: "toolCall", id: "call-1", name: "clock", arguments: {} }],
    };
    expect(map({ type: "toolcall_start", contentIndex: 0, partial })).toStrictEqual({
      version: 1,
      type: "toolcall_start",
      id: "request",
      index: 0,
      toolCallId: "call-1",
      name: "clock",
    });
    expect(
      map({ type: "toolcall_delta", contentIndex: 0, delta: '{"zone":', partial }),
    ).toStrictEqual({
      version: 1,
      type: "toolcall_delta",
      id: "request",
      index: 0,
      delta: '{"zone":',
    });
    expect(
      map({
        type: "toolcall_end",
        contentIndex: 0,
        toolCall: { type: "toolCall", id: "call-1", name: "clock", arguments: { zone: "UTC" } },
        partial,
      }),
    ).toStrictEqual({
      version: 1,
      type: "toolcall_end",
      id: "request",
      index: 0,
      toolCallId: "call-1",
      name: "clock",
      arguments: { zone: "UTC" },
    });
  });

  it("maps full authoritative done and error messages", () => {
    const done = { ...MESSAGE, stopReason: "stop" as const };
    const failed = { ...MESSAGE, stopReason: "error" as const, errorMessage: "failed" };
    expect(map({ type: "done", reason: "stop", message: done })).toStrictEqual({
      version: 1,
      type: "done",
      id: "request",
      reason: "stop",
      message: done,
    });
    expect(map({ type: "error", reason: "error", error: failed })).toStrictEqual({
      version: 1,
      type: "error",
      id: "request",
      reason: "error",
      error: failed,
    });
  });

  it("preserves rich authoritative terminal messages without modification", () => {
    const richMessage: AssistantMessage = {
      ...MESSAGE,
      content: [
        { type: "text", text: "answer", textSignature: "text-signature" },
        {
          type: "thinking",
          thinking: "[Reasoning redacted]",
          thinkingSignature: "thinking-signature",
          redacted: true,
        },
        {
          type: "toolCall",
          id: "call-rich",
          name: "lookup",
          arguments: { query: "value" },
          thoughtSignature: "tool-signature",
          namespace: "dynamic-tools",
        },
      ],
      responseModel: "resolved-model",
      responseId: "response-1",
      usage: {
        input: 11,
        output: 7,
        cacheRead: 5,
        cacheWrite: 3,
        cacheWrite1h: 2,
        reasoning: 4,
        totalTokens: 26,
        cost: { input: 1, output: 2, cacheRead: 3, cacheWrite: 4, total: 10 },
      },
      stopReason: "deferred",
      deferred: {
        provider: "provider",
        modelId: "model",
        api: "openai-responses",
        id: "deferred-1",
        expiresAt: 123,
        pollAfterMs: 456,
        data: { cursor: "next" },
      },
      rawStopReason: "queued",
      endTurn: false,
    };

    const mapped = map({ type: "done", reason: "deferred", message: richMessage });
    expect(mapped).toStrictEqual({
      version: 1,
      type: "done",
      id: "request",
      reason: "deferred",
      message: richMessage,
    });
    expect(mapped["message"]).toBe(richMessage);
  });

  it("rejects a malformed tool-call start", () => {
    expect(() => map({ type: "toolcall_start", contentIndex: 0, partial: MESSAGE })).toThrow(
      "has no tool call",
    );
  });
});
