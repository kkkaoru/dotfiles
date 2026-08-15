import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import type { DirectStreamInput } from "../src/direct-stream.ts";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { GatewayConnection } from "../src/gateway.ts";
import {
  parseClientMessage,
  type ServerMessage,
  type StreamRequestMessage,
} from "../src/protocol.ts";

const streamMock = vi.hoisted(() => vi.fn<(input: DirectStreamInput) => Promise<void>>());
vi.mock("../src/direct-stream.ts", () => ({ streamDirectModel: streamMock }));

const TOKEN = "01234567890123456789012345678901";
const MODEL = {
  provider: "ollama-cloud",
  id: "glm-5.2",
  name: "GLM",
  api: "openai-completions",
  reasoning: true,
  input: ["text"],
  contextWindow: 100,
  maxTokens: 10,
};

function request(id = "request"): StreamRequestMessage {
  const parsed = parseClientMessage(
    JSON.stringify({
      version: 1,
      type: "request",
      id,
      token: TOKEN,
      origin: "claudex",
      provider: MODEL.provider,
      modelId: MODEL.id,
      system: null,
      messages: [],
      tools: [],
      options: {},
    }),
  );
  if (parsed.type !== "request") {
    throw new Error("invalid fixture");
  }
  return parsed;
}

function registry() {
  return {
    getAvailable: () => [MODEL, { ...MODEL, provider: "claudex", id: "loop" }],
  } as unknown as ExtensionContext["modelRegistry"];
}

async function settle() {
  await new Promise<void>((resolve) => {
    setTimeout(resolve, 0);
  });
}

beforeEach(() => {
  streamMock.mockReset();
  streamMock.mockResolvedValue(undefined);
});

describe("gateway request coordination", () => {
  it("lists available models while excluding the claudex provider", async () => {
    const messages: ServerMessage[] = [];
    const gateway = new GatewayConnection(registry(), {
      write: async (message) => {
        messages.push(message);
      },
    });
    gateway.handle({ version: 1, type: "list_models", id: "models", token: TOKEN });
    await settle();
    expect(messages).toStrictEqual([
      {
        version: 1,
        type: "models",
        id: "models",
        models: [MODEL],
      },
    ]);
  });

  it("observes asynchronous model-list writer failures", async () => {
    const write = vi.fn(async () => {
      throw new Error("closed");
    });
    const gateway = new GatewayConnection(registry(), { write });
    gateway.handle({ version: 1, type: "list_models", id: "models", token: TOKEN });
    await settle();
    expect(write).toHaveBeenCalledTimes(1);
  });

  it("maps streamed events and forwards a full terminal message", async () => {
    const messages: ServerMessage[] = [];
    const final = {
      role: "assistant" as const,
      content: [],
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
      stopReason: "stop" as const,
      timestamp: 1,
    };
    streamMock.mockImplementation(async (input) => {
      await input.onEvent({ type: "start", partial: final });
      await input.onEvent({ type: "done", reason: "stop", message: final });
    });
    const gateway = new GatewayConnection(registry(), {
      write: async (message) => {
        messages.push(message);
      },
    });
    gateway.handle(request());
    await settle();
    expect(messages.map((message) => message.type)).toStrictEqual(["start", "done"]);
    expect(messages[1]?.["message"]).toStrictEqual(final);
  });

  it("rejects duplicate ids and unknown cancellation", async () => {
    let release: (() => void) | undefined;
    streamMock.mockImplementation(
      async () =>
        new Promise<void>((resolve) => {
          release = resolve;
        }),
    );
    const messages: ServerMessage[] = [];
    const gateway = new GatewayConnection(registry(), {
      write: async (message) => {
        messages.push(message);
      },
    });
    gateway.handle(request());
    gateway.handle(request());
    gateway.handle({ version: 1, type: "cancel", id: "missing", token: TOKEN });
    await settle();
    expect(messages).toStrictEqual([
      {
        version: 1,
        type: "protocol_error",
        id: "request",
        message: "Duplicate active request id: request",
      },
      {
        version: 1,
        type: "protocol_error",
        id: "missing",
        message: "No active request for cancellation: missing",
      },
    ]);
    release?.();
  });

  it("aborts active requests on cancel and connection close", async () => {
    const signals: AbortSignal[] = [];
    streamMock.mockImplementation(async (input) => {
      signals.push(input.signal);
      await new Promise<void>((resolve) => {
        input.signal.addEventListener("abort", () => {
          resolve();
        });
      });
    });
    const gateway = new GatewayConnection(registry(), { write: async () => {} });
    gateway.handle(request("first"));
    await settle();
    gateway.handle({ version: 1, type: "cancel", id: "first", token: TOKEN });
    gateway.handle(request("second"));
    await settle();
    gateway.close();
    gateway.handle(request("ignored"));
    expect(signals.map((signal) => signal.aborted)).toStrictEqual([true, true]);
    expect(streamMock).toHaveBeenCalledTimes(2);
  });

  it("turns pre-terminal failures into protocol errors without duplicating terminal events", async () => {
    const messages: ServerMessage[] = [];
    streamMock.mockRejectedValueOnce(new Error("provider exploded"));
    const gateway = new GatewayConnection(registry(), {
      write: async (message) => {
        messages.push(message);
      },
    });
    gateway.handle(request("failed"));
    await settle();
    expect(messages).toStrictEqual([
      {
        version: 1,
        type: "protocol_error",
        id: "failed",
        message: "provider exploded",
      },
    ]);

    streamMock.mockRejectedValueOnce("plain failure");
    gateway.handle(request("plain"));
    await settle();
    expect(messages[1]).toStrictEqual({
      version: 1,
      type: "protocol_error",
      id: "plain",
      message: "plain failure",
    });
  });

  it("does not emit a second terminal error after provider termination", async () => {
    const messages: ServerMessage[] = [];
    const final = {
      role: "assistant" as const,
      content: [],
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
      stopReason: "stop" as const,
      timestamp: 1,
    };
    streamMock.mockImplementation(async (input) => {
      await input.onEvent({ type: "done", reason: "stop", message: final });
      await input.onEvent({ type: "text_start", contentIndex: 0, partial: final });
    });
    const gateway = new GatewayConnection(registry(), {
      write: async (message) => {
        messages.push(message);
      },
    });
    gateway.handle(request("terminal"));
    await settle();
    expect(messages).toStrictEqual([
      { version: 1, type: "done", id: "terminal", reason: "stop", message: final },
    ]);
  });
});

describe("web_search handler", () => {
  it("falls back to Exa API for non-cursor providers", async () => {
    const originalKey = process.env["EXA_API_KEY"];
    delete process.env["EXA_API_KEY"];
    try {
      const messages: ServerMessage[] = [];
      const reg = {
        getAvailable: () => [MODEL],
        find: (provider: string, modelId: string) =>
          provider === MODEL.provider && modelId === MODEL.id ? MODEL : undefined,
      } as unknown as ExtensionContext["modelRegistry"];
      const gateway = new GatewayConnection(reg, {
        write: async (message) => {
          messages.push(message);
        },
      });
      gateway.handle({
        version: 1,
        type: "web_search",
        id: "ws1",
        token: TOKEN,
        provider: MODEL.provider,
        modelId: MODEL.id,
        query: "test query",
      });
      await settle();
      expect(messages).toHaveLength(1);
      expect(messages[0]?.type).toBe("web_search_error");
      expect((messages[0] as Record<string, unknown>)["message"]).toContain("EXA_API_KEY");
    } finally {
      if (originalKey !== undefined) {
        process.env["EXA_API_KEY"] = originalKey;
      }
    }
  });

  it("returns error when model is not found", async () => {
    const messages: ServerMessage[] = [];
    const reg = {
      getAvailable: () => [],
      find: () => undefined,
    } as unknown as ExtensionContext["modelRegistry"];
    const gateway = new GatewayConnection(reg, {
      write: async (message) => {
        messages.push(message);
      },
    });
    gateway.handle({
      version: 1,
      type: "web_search",
      id: "ws2",
      token: TOKEN,
      provider: "nonexistent",
      modelId: "missing",
      query: "test",
    });
    await settle();
    expect(messages).toHaveLength(1);
    expect(messages[0]?.type).toBe("web_search_error");
    expect((messages[0] as Record<string, unknown>)["message"]).toContain("Model not found");
  });

  it("calls complete() and parses results for cursor provider", async () => {
    const cursorModel = { ...MODEL, provider: "cursor", id: "auto" };
    const completeMock = vi.fn().mockResolvedValue({
      content: [
        {
          type: "text",
          text: ["Title: Test Result", "URL: https://example.com", "Snippet: A test snippet"].join(
            "\n",
          ),
        },
      ],
    });
    const reg = {
      getAvailable: () => [cursorModel],
      find: (provider: string, modelId: string) =>
        provider === "cursor" && modelId === "auto" ? cursorModel : undefined,
      complete: completeMock,
    } as unknown as ExtensionContext["modelRegistry"];
    const messages: ServerMessage[] = [];
    const gateway = new GatewayConnection(reg, {
      write: async (message) => {
        messages.push(message);
      },
    });
    gateway.handle({
      version: 1,
      type: "web_search",
      id: "ws3",
      token: TOKEN,
      provider: "cursor",
      modelId: "auto",
      query: "bitcoin price",
    });
    await settle();
    expect(completeMock).toHaveBeenCalledTimes(1);
    expect(messages).toHaveLength(1);
    expect(messages[0]?.type).toBe("web_search_result");
    const result = messages[0] as Record<string, unknown>;
    expect(result["provider"]).toBe("cursor");
    expect(result["modelId"]).toBe("auto");
    const results = result["results"] as { title: string; url: string; snippet: string }[];
    expect(results).toHaveLength(1);
    expect(results[0]?.title).toBe("Test Result");
    expect(results[0]?.url).toBe("https://example.com");
  });
});

it("returns error when cursor complete() throws", async () => {
  const cursorModel = { ...MODEL, provider: "cursor", id: "auto" };
  const completeMock = vi.fn().mockRejectedValue(new Error("cursor api down"));
  const reg = {
    getAvailable: () => [cursorModel],
    find: (prov: string, mid: string) =>
      prov === "cursor" && mid === "auto" ? cursorModel : undefined,
    complete: completeMock,
  } as unknown as ExtensionContext["modelRegistry"];
  const messages: ServerMessage[] = [];
  const gw = new GatewayConnection(reg, {
    write: async (msg) => {
      messages.push(msg);
    },
  });
  gw.handle({
    version: 1,
    type: "web_search",
    id: "ws-cursor-err",
    token: TOKEN,
    provider: "cursor",
    modelId: "auto",
    query: "test",
  });
  await settle();
  expect(messages).toHaveLength(1);
  expect(messages[0]?.type).toBe("web_search_error");
  expect((messages[0] as Record<string, unknown>)["message"]).toContain("cursor api down");
});
