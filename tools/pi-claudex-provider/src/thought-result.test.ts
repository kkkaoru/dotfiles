// Runs with Bun.

import type { AssistantMessage, ThinkingContent } from "@earendil-works/pi-ai";
import { describe, expect, it } from "vitest";
import { normalizeThoughtResult } from "./thought-result.ts";

function message(api: AssistantMessage["api"], thinkingSignature?: string): AssistantMessage {
  const block: ThinkingContent =
    thinkingSignature === undefined
      ? { type: "thinking", thinking: "private" }
      : { type: "thinking", thinking: "private", thinkingSignature };
  return {
    role: "assistant",
    content: [block],
    api,
    provider: "provider",
    model: "model",
    usage: {
      input: 0,
      output: 0,
      cacheRead: 0,
      cacheWrite: 0,
      totalTokens: 0,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
    },
    stopReason: "stop",
    timestamp: 1,
  };
}

describe("thought result normalization", () => {
  it("compacts and bounds the last raw paragraph", () => {
    const longResult = `private\n\n${"x".repeat(405)}`;
    expect(normalizeThoughtResult(message("anthropic-messages"), 0, longResult)).toBe(
      `…${"x".repeat(399)}`,
    );
    expect(normalizeThoughtResult(message("anthropic-messages"), 0, "a\n\n  final\nresult ")).toBe(
      "final result",
    );
  });

  it("falls back when a Responses signature has no usable summary", () => {
    const arraySignature = JSON.stringify([]);
    const invalidSummarySignature = JSON.stringify({ summary: "invalid" });
    expect(normalizeThoughtResult(message("openai-responses"), 0, "raw\n\nfallback")).toBe(
      "fallback",
    );
    expect(normalizeThoughtResult(message("openai-responses", "not-json"), 0, "fallback")).toBe(
      "fallback",
    );
    expect(normalizeThoughtResult(message("openai-responses", arraySignature), 0, "fallback")).toBe(
      "fallback",
    );
    expect(
      normalizeThoughtResult(message("openai-responses", invalidSummarySignature), 0, "fallback"),
    ).toBe("fallback");
  });

  it("ignores unusable native summary parts", () => {
    const signature = JSON.stringify({
      summary: [null, { text: " " }, { text: "first" }, { nope: "missing" }, { text: "final" }],
    });
    expect(normalizeThoughtResult(message("azure-openai-responses", signature), 0, "raw")).toBe(
      "final",
    );
  });

  it("handles empty and missing thinking blocks", () => {
    expect(normalizeThoughtResult(message("openai-codex-responses", "{}"), 0, "")).toBe("");
    expect(normalizeThoughtResult(message("openai-responses", "{}"), 2, "terminal")).toBe(
      "terminal",
    );
  });
});
