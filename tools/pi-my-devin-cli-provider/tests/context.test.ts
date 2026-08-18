// This file runs with Bun.
import type { Context } from "@earendil-works/pi-ai";
import { expect, test } from "vitest";
import {
  buildContinuationPrompt,
  buildDevinTranscript,
  latestUserText,
  transcriptIncludesCompaction,
} from "../src/context.ts";

test("serializes system, text, images, thinking, and tool history", () => {
  const context: Context = {
    systemPrompt: "Follow rules",
    messages: [
      {
        role: "user",
        content: [
          { type: "text", text: "inspect" },
          { type: "image", data: "data", mimeType: "image/png" },
        ],
        timestamp: 1,
      },
      {
        role: "assistant",
        content: [
          { type: "thinking", thinking: "reason" },
          { type: "toolCall", id: "call-1", name: "read", arguments: { path: "a" } },
        ],
        api: "devin-cli-acp",
        provider: "devin",
        model: "adaptive",
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

  expect(buildDevinTranscript(context)).toBe(
    [
      "SYSTEM INSTRUCTIONS:\nFollow rules",
      "USER:\ninspect\n[image: image/png]",
      'ASSISTANT:\n[thinking]\nreason\n[tool call call-1: read]\n{"path":"a"}',
      "TOOL RESULT (read, call-1, error):\ncontents",
      "Continue from the transcript above. Follow the latest user request.",
    ].join("\n\n"),
  );
});

test("serializes a plain user message without a system prompt", () => {
  expect(
    buildDevinTranscript({
      messages: [{ role: "user", content: "hello", timestamp: 1 }],
    }),
  ).toBe("USER:\nhello\n\nContinue from the transcript above. Follow the latest user request.");
});

test("uses the latest user text for continuing Devin sessions", () => {
  const context: Context = {
    systemPrompt: "Follow rules",
    messages: [
      { role: "user", content: "first", timestamp: 1 },
      {
        role: "assistant",
        content: [{ type: "text", text: "ok" }],
        api: "devin-cli-acp",
        provider: "devin",
        model: "adaptive",
        usage: {
          input: 0,
          output: 0,
          cacheRead: 0,
          cacheWrite: 0,
          totalTokens: 0,
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
        },
        stopReason: "stop",
        timestamp: 2,
      },
      { role: "user", content: "second", timestamp: 3 },
    ],
  };
  expect(latestUserText(context)).toBe("second");
  expect(buildContinuationPrompt(context)).toBe("second");
  expect(buildContinuationPrompt({ messages: [] })).toBe(
    "Continue from the transcript above. Follow the latest user request.",
  );
});

test("detects pi compaction summary markers in transcripts", () => {
  const compacted = buildDevinTranscript({
    messages: [
      {
        role: "user",
        content:
          "The conversation history before this point was compacted into the following summary:\n\n<summary>\nok\n</summary>",
        timestamp: 1,
      },
    ],
  });
  expect(transcriptIncludesCompaction(compacted)).toBe(true);
  expect(transcriptIncludesCompaction("USER:\nhello")).toBe(false);
});
