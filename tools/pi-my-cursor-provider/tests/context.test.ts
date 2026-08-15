import type { Context, ToolResultMessage } from "@earendil-works/pi-ai";
import { expect, test } from "vitest";
import {
  buildCursorMessage,
  findToolResults,
  toolResultToSdk,
  toSdkJsonValue,
} from "../src/context.ts";

test("serializes the complete transcript and latest user images", () => {
  const context: Context = {
    systemPrompt: "Follow rules",
    messages: [
      {
        role: "user",
        content: [
          { type: "text", text: "inspect" },
          { type: "image", data: "aGVsbG8=", mimeType: "image/png" },
        ],
        timestamp: 1,
      },
      {
        role: "assistant",
        content: [
          { type: "thinking", thinking: "reason" },
          { type: "toolCall", id: "call-1", name: "read", arguments: { path: "a" } },
        ],
        api: "cursor-agent",
        provider: "cursor",
        model: "auto",
        usage: {
          input: 0,
          output: 0,
          cacheRead: 0,
          cacheWrite: 0,
          totalTokens: 0,
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
        },
        stopReason: "toolUse",
        timestamp: 2,
      },
      {
        role: "toolResult",
        toolCallId: "call-1",
        toolName: "read",
        content: [{ type: "text", text: "contents" }],
        isError: true,
        timestamp: 3,
      },
    ],
  };

  expect(buildCursorMessage(context)).toStrictEqual({
    text: [
      "SYSTEM INSTRUCTIONS:\nFollow rules",
      "USER:\ninspect\n[image attached]",
      'ASSISTANT:\n[thinking]\nreason\n[tool call call-1: read]\n{"path":"a"}',
      "TOOL RESULT (read, call-1, error):\ncontents",
      "Continue from the transcript above. Follow the latest user request.",
    ].join("\n\n"),
    images: [{ data: "aGVsbG8=", mimeType: "image/png" }],
  });
});

test("serializes plain user text without optional fields", () => {
  expect(
    buildCursorMessage({ messages: [{ role: "user", content: "hello", timestamp: 1 }] }),
  ).toStrictEqual({
    text: "USER:\nhello\n\nContinue from the transcript above. Follow the latest user request.",
  });
});

test("finds tool results and converts mixed content", () => {
  const result: ToolResultMessage = {
    role: "toolResult",
    toolCallId: "call-1",
    toolName: "vision",
    content: [
      { type: "text", text: "ok" },
      { type: "image", data: "data", mimeType: "image/jpeg" },
    ],
    isError: false,
    timestamp: 2,
  };
  const context: Context = {
    messages: [{ role: "user", content: "hello", timestamp: 1 }, result],
  };

  expect(findToolResults(context)).toStrictEqual([result]);
  expect(toolResultToSdk(result)).toStrictEqual({
    content: [
      { type: "text", text: "ok" },
      { type: "image", data: "data", mimeType: "image/jpeg" },
    ],
    isError: false,
  });
});

test("normalizes only finite JSON-compatible values", () => {
  expect(toSdkJsonValue({ a: [1, true, null, "x"] })).toStrictEqual({
    a: [1, true, null, "x"],
  });
  expect(toSdkJsonValue(Number.NaN)).toBeUndefined();
  expect(toSdkJsonValue([undefined])).toBeUndefined();
  expect(toSdkJsonValue({ bad: undefined })).toBeUndefined();
  expect(toSdkJsonValue(Symbol("bad"))).toBeUndefined();
});
