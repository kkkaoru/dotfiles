// Runs with Bun.

import type { AssistantMessage, AssistantMessageEvent, ToolCall } from "@earendil-works/pi-ai";
import { afterEach, describe, expect, it, vi } from "vitest";
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

afterEach(() => {
  vi.useRealTimers();
});

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
      type: "thinking_progress",
      id: "request",
      index: 1,
      deltaChars: 3,
    });
    expect(
      map({
        type: "thinking_end",
        contentIndex: 1,
        content: "private steps\n\nThe edit is ready.",
        partial: MESSAGE,
      }),
    ).toStrictEqual({
      version: 1,
      type: "thinking_result",
      id: "request",
      index: 1,
      result: "The edit is ready.",
    });
  });

  it("counts Unicode code points without exposing private reasoning", () => {
    expect(
      map({ type: "thinking_delta", contentIndex: 1, delta: "考🧠", partial: MESSAGE }),
    ).toStrictEqual({
      version: 1,
      type: "thinking_progress",
      id: "request",
      index: 1,
      deltaChars: 2,
    });
  });

  it("prefers a native Responses reasoning summary", () => {
    const signature = JSON.stringify({
      type: "reasoning",
      summary: [{ type: "summary_text", text: "Checked files.\n\nUse the shared protocol." }],
      content: [{ type: "reasoning_text", text: "private chain" }],
    });
    const partial: AssistantMessage = {
      ...MESSAGE,
      api: "openai-responses",
      content: [{ type: "thinking", thinking: "private chain", thinkingSignature: signature }],
    };

    expect(
      map({ type: "thinking_end", contentIndex: 0, content: "private chain", partial }),
    ).toStrictEqual({
      version: 1,
      type: "thinking_result",
      id: "request",
      index: 0,
      result: "Use the shared protocol.",
    });
  });

  it("returns no result for redacted thinking", () => {
    const partial: AssistantMessage = {
      ...MESSAGE,
      content: [{ type: "thinking", thinking: "[redacted]", redacted: true }],
    };
    expect(
      map({ type: "thinking_end", contentIndex: 0, content: "[redacted]", partial }),
    ).toMatchObject({ type: "thinking_result", result: "" });
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

  it("backgrounds long Bash calls in incremental and terminal events", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 7, 25, 23, 14, 59));
    const toolCall: ToolCall = {
      type: "toolCall",
      id: "bash-1",
      name: "Bash",
      arguments: { command: "gh run watch 123" },
    };
    const partial: AssistantMessage = { ...MESSAGE, content: [toolCall] };

    expect(map({ type: "toolcall_end", contentIndex: 0, toolCall, partial })).toMatchObject({
      arguments: {
        command: "gh run watch 123",
        description: "08-25 23:14 | gh run watch 123",
        run_in_background: true,
      },
    });
    expect(map({ type: "done", reason: "toolUse", message: partial })).toMatchObject({
      message: {
        content: [
          {
            arguments: {
              command: "gh run watch 123",
              description: "08-25 23:14 | gh run watch 123",
              run_in_background: true,
            },
          },
        ],
      },
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
      terminal: { state: "recoverable_error", output: "none", code: "empty_assistant" },
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
      terminal: { state: "complete", output: "tool_use" },
    });
    expect(mapped["message"]).toBe(richMessage);
  });

  it("normalizes provider-specific empty tool termination", () => {
    const emptyToolUse: AssistantMessage = {
      ...MESSAGE,
      stopReason: "toolUse",
      content: [{ type: "thinking", thinking: "unfinished tool arguments" }],
    };
    expect(map({ type: "done", reason: "toolUse", message: emptyToolUse })).toStrictEqual({
      version: 1,
      type: "done",
      id: "request",
      reason: "toolUse",
      message: emptyToolUse,
      terminal: {
        state: "recoverable_error",
        output: "none",
        code: "tool_use_without_call",
      },
    });
  });

  it("marks visible assistant output as complete", () => {
    const answer: AssistantMessage = {
      ...MESSAGE,
      stopReason: "stop",
      content: [{ type: "text", text: "answer" }],
    };
    expect(map({ type: "done", reason: "stop", message: answer })["terminal"]).toStrictEqual({
      state: "complete",
      output: "assistant",
    });
  });

  it("normalizes an empty length termination as recoverable", () => {
    const truncated: AssistantMessage = { ...MESSAGE, stopReason: "length" };
    expect(map({ type: "done", reason: "length", message: truncated })["terminal"]).toStrictEqual({
      state: "recoverable_error",
      output: "none",
      code: "empty_assistant",
    });
  });

  it("normalizes empty length, deferred, and zero-width terminal output", () => {
    const zeroWidth: AssistantMessage = {
      ...MESSAGE,
      content: [{ type: "text", text: "\u200B " }],
      stopReason: "length",
    };
    expect(map({ type: "done", reason: "length", message: zeroWidth })["terminal"]).toStrictEqual({
      state: "recoverable_error",
      output: "none",
      code: "empty_assistant",
    });

    const deferred: AssistantMessage = { ...MESSAGE, stopReason: "deferred" };
    expect(map({ type: "done", reason: "deferred", message: deferred })["terminal"]).toStrictEqual({
      state: "complete",
      output: "none",
    });
  });

  it("rejects a malformed tool-call start", () => {
    expect(() => map({ type: "toolcall_start", contentIndex: 0, partial: MESSAGE })).toThrow(
      "has no tool call",
    );
  });
});
