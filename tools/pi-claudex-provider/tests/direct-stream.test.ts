import type { Api, AssistantMessage, AssistantMessageEvent, Model } from "@earendil-works/pi-ai";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { describe, expect, it, vi } from "vitest";
import { streamDirectModel } from "../src/direct-stream.ts";
import { parseClientMessage, type StreamRequestMessage } from "../src/protocol.ts";

const TOKEN = "01234567890123456789012345678901";
const MODEL: Model<Api> = {
  provider: "ollama-cloud",
  id: "glm-5.2",
  name: "GLM",
  api: "openai-completions",
  baseUrl: "https://old.test",
  reasoning: true,
  input: ["text"],
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
  contextWindow: 1000,
  maxTokens: 100,
};
const FINAL: AssistantMessage = {
  role: "assistant",
  content: [{ type: "text", text: "ok" }],
  api: MODEL.api,
  provider: MODEL.provider,
  model: MODEL.id,
  usage: {
    input: 1,
    output: 1,
    cacheRead: 0,
    cacheWrite: 0,
    totalTokens: 2,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
  },
  stopReason: "stop",
  timestamp: 1,
};

function request(options: Record<string, unknown> = {}): StreamRequestMessage {
  const parsed = parseClientMessage(
    JSON.stringify({
      version: 1,
      type: "request",
      id: "request",
      token: TOKEN,
      origin: "claudex",
      provider: MODEL.provider,
      modelId: MODEL.id,
      system: "system",
      messages: [{ role: "user", content: "hello" }],
      tools: [],
      options,
    }),
  );
  if (parsed.type !== "request") {
    throw new Error("invalid fixture");
  }
  return parsed;
}

async function* events(values: AssistantMessageEvent[]) {
  for (const value of values) {
    yield value;
  }
}

interface StreamCall {
  model: unknown;
  context: unknown;
  options: unknown;
}

function registry(overrides: Record<string, unknown> = {}) {
  const calls: StreamCall[] = [];
  const streamSimple = vi.fn((model: unknown, context: unknown, options?: unknown) => {
    calls.push({ model, context, options });
    return events([
      { type: "start", partial: FINAL },
      { type: "done", reason: "stop", message: FINAL },
    ]);
  });
  const value = {
    getAll: () => [MODEL],
    getAvailable: () => [MODEL],
    getProvider: () => ({ streamSimple }),
    getApiKeyAndHeaders: async () => ({
      ok: true,
      apiKey: "key",
      headers: { "x-test": "yes" },
      baseUrl: "https://resolved.test",
      env: { REGION: "test" },
    }),
    ...overrides,
  };
  return { calls, streamSimple, value: value as unknown as ExtensionContext["modelRegistry"] };
}

describe("direct Pi provider streaming", () => {
  it("resolves auth, converts context, streams events, and applies options", async () => {
    const harness = registry();
    const received: string[] = [];
    await streamDirectModel({
      request: request({
        reasoning: "high",
        maxTokens: 50,
        temperature: 0.1,
        metadata: { user_id: "u" },
        sessionId: "s",
        cacheRetention: "long",
      }),
      registry: harness.value,
      signal: new AbortController().signal,
      onEvent: async (event) => {
        received.push(event.type);
      },
    });
    expect(received).toStrictEqual(["start", "done"]);
    expect(harness.streamSimple).toHaveBeenCalledTimes(1);
    expect(harness.calls[0]?.model).toMatchObject({
      provider: MODEL.provider,
      id: MODEL.id,
      baseUrl: "https://resolved.test",
    });
    expect(harness.calls[0]?.context).toMatchObject({
      systemPrompt: "system",
      messages: [{ role: "user", content: "hello" }],
    });
    expect(harness.calls[0]?.options).toMatchObject({
      apiKey: "key",
      headers: { "x-test": "yes" },
      env: { REGION: "test" },
      reasoning: "high",
      maxTokens: 50,
      temperature: 0.1,
      metadata: { user_id: "u" },
      sessionId: "s",
      cacheRetention: "long",
    });
  });

  it("omits off reasoning and optional auth values", async () => {
    const harness = registry({
      getApiKeyAndHeaders: async () => ({ ok: true }),
    });
    await streamDirectModel({
      request: request({ reasoning: "off" }),
      registry: harness.value,
      signal: new AbortController().signal,
      onEvent: async () => {},
    });
    expect(harness.calls[0]?.options).not.toHaveProperty("reasoning");
    expect(harness.calls[0]?.model).toMatchObject({ baseUrl: MODEL.baseUrl });
  });

  it("rejects unavailable models, missing providers, and failed auth", async () => {
    await expect(
      streamDirectModel({
        request: request(),
        registry: registry({ getAll: () => [], getAvailable: () => [] }).value,
        signal: new AbortController().signal,
        onEvent: async () => {},
      }),
    ).rejects.toThrow("model is unavailable");
    await expect(
      streamDirectModel({
        request: request(),
        registry: registry({ getProvider: () => undefined }).value,
        signal: new AbortController().signal,
        onEvent: async () => {},
      }),
    ).rejects.toThrow("provider not found");
    await expect(
      streamDirectModel({
        request: request(),
        registry: registry({ getApiKeyAndHeaders: async () => ({ ok: false, error: "login" }) })
          .value,
        signal: new AbortController().signal,
        onEvent: async () => {},
      }),
    ).rejects.toThrow("authentication failed: login");
  });

  it("requires exactly one terminal provider event", async () => {
    const noTerminal = registry({
      getProvider: () => ({ streamSimple: () => events([{ type: "start", partial: FINAL }]) }),
    });
    await expect(
      streamDirectModel({
        request: request(),
        registry: noTerminal.value,
        signal: new AbortController().signal,
        onEvent: async () => {},
      }),
    ).rejects.toThrow("without a terminal event");

    const failed = { ...FINAL, stopReason: "error" as const, errorMessage: "failed" };
    const errorTerminal = registry({
      getProvider: () => ({
        streamSimple: () => events([{ type: "error", reason: "error", error: failed }]),
      }),
    });
    await expect(
      streamDirectModel({
        request: request(),
        registry: errorTerminal.value,
        signal: new AbortController().signal,
        onEvent: async () => {},
      }),
    ).resolves.toBeUndefined();
  });
});
