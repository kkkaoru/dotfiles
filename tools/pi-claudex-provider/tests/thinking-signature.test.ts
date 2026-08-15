import type { Api, Model } from "@earendil-works/pi-ai";
import { describe, expect, it } from "vitest";
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
  input: ["text"],
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
  contextWindow: 1000,
  maxTokens: 100,
};

function gatewayRequest(signature?: string): StreamRequestMessage {
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
      messages: [
        {
          role: "assistant",
          content: [
            {
              type: "thinking",
              thinking: "consider",
              ...(signature === undefined ? {} : { signature }),
            },
          ],
        },
      ],
      tools: [],
      options: {},
    }),
  );
  if (message.type !== "request") {
    throw new Error("fixture did not parse as request");
  }
  return message;
}

function convertedThinking(signature?: string) {
  const context = toPiContext(gatewayRequest(signature), MODEL);
  const message = context.messages[0];
  if (message?.role !== "assistant") {
    throw new Error("expected assistant thinking message");
  }
  return message.content;
}

describe("adapter thinking signature replay", () => {
  it("drops every adapter-synthesized signature before Pi replay", () => {
    expect(convertedThinking()).toStrictEqual([{ type: "thinking", thinking: "consider" }]);
    expect(convertedThinking("claudex_local_0123456789abcdef0123456789abcdef")).toStrictEqual([
      { type: "thinking", thinking: "consider" },
    ]);
    expect(convertedThinking("claudex_activity_keepalive")).toStrictEqual([
      { type: "thinking", thinking: "consider" },
    ]);
    expect(convertedThinking("claudex_provider_progress")).toStrictEqual([
      { type: "thinking", thinking: "consider" },
    ]);
    expect(convertedThinking("claudex_future_marker")).toStrictEqual([
      { type: "thinking", thinking: "consider" },
    ]);
  });

  it("keeps Completions opaque signatures used by Grok, Cursor, and Ollama", () => {
    expect(convertedThinking("sig")).toStrictEqual([
      { type: "thinking", thinking: "consider", thinkingSignature: "sig" },
    ]);
    expect(convertedThinking("reasoning_content")).toStrictEqual([
      { type: "thinking", thinking: "consider", thinkingSignature: "reasoning_content" },
    ]);
  });

  it("keeps Codex JSON reasoning items for GPT, Luna, and Spark replay", () => {
    const codexSignature =
      '{"id":"rs_1","type":"reasoning","encrypted_content":"abc","summary":[]}';
    expect(convertedThinking(codexSignature)).toStrictEqual([
      { type: "thinking", thinking: "consider", thinkingSignature: codexSignature },
    ]);
  });
});
