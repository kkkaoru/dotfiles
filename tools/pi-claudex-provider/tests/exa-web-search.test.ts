import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { describe, expect, it, vi } from "vitest";
import { GatewayConnection } from "../src/gateway.ts";
import type { ServerMessage } from "../src/protocol.ts";

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

async function settle() {
  await new Promise<void>((resolve) => {
    setTimeout(resolve, 0);
  });
}

async function withExaKey(key: string, fn: () => Promise<void>): Promise<void> {
  const originalKey = process.env["EXA_API_KEY"];
  const originalFetch = globalThis.fetch;
  process.env["EXA_API_KEY"] = key;
  try {
    await fn();
  } finally {
    globalThis.fetch = originalFetch;
    if (originalKey === undefined) {
      delete process.env["EXA_API_KEY"];
    } else {
      process.env["EXA_API_KEY"] = originalKey;
    }
  }
}

describe("exa web search", () => {
  it("calls Exa API and returns results for non-cursor providers", async () => {
    globalThis.fetch = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => ({
        results: [{ title: "Exa Result", url: "https://exa.example.com", text: "Exa snippet" }],
      }),
    });
    await withExaKey("test-exa-key", async () => {
      const messages: ServerMessage[] = [];
      const reg = {
        getAvailable: () => [MODEL],
        find: (prov: string, mid: string) =>
          prov === MODEL.provider && mid === MODEL.id ? MODEL : undefined,
      } as unknown as ExtensionContext["modelRegistry"];
      const gateway = new GatewayConnection(reg, {
        write: async (msg) => {
          messages.push(msg);
        },
      });
      gateway.handle({
        version: 1,
        type: "web_search",
        id: "ws-exa",
        token: TOKEN,
        provider: MODEL.provider,
        modelId: MODEL.id,
        query: "bitcoin price",
      });
      await settle();
      expect(globalThis.fetch).toHaveBeenCalledTimes(1);
      expect(messages).toHaveLength(1);
      expect(messages[0]?.type).toBe("web_search_result");
      const res = messages[0] as Record<string, unknown>;
      const items = res["results"] as { title: string; url: string; snippet: string }[];
      expect(items).toHaveLength(1);
      expect(items[0]?.title).toBe("Exa Result");
    });
  });

  it("returns error when Exa API responds with non-OK status", async () => {
    globalThis.fetch = vi.fn().mockResolvedValue({
      ok: false,
      status: 401,
      text: async () => "Unauthorized",
    });
    await withExaKey("test-exa-key", async () => {
      const messages: ServerMessage[] = [];
      const reg = {
        getAvailable: () => [MODEL],
        find: (prov: string, mid: string) =>
          prov === MODEL.provider && mid === MODEL.id ? MODEL : undefined,
      } as unknown as ExtensionContext["modelRegistry"];
      const gateway = new GatewayConnection(reg, {
        write: async (msg) => {
          messages.push(msg);
        },
      });
      gateway.handle({
        version: 1,
        type: "web_search",
        id: "ws-exa-err",
        token: TOKEN,
        provider: MODEL.provider,
        modelId: MODEL.id,
        query: "test",
      });
      await settle();
      expect(messages).toHaveLength(1);
      expect(messages[0]?.type).toBe("web_search_error");
      expect((messages[0] as Record<string, unknown>)["message"]).toContain("401");
    });
  });
});

it("returns error when Exa API fetch throws", async () => {
  globalThis.fetch = vi.fn().mockRejectedValue(new Error("network failure"));
  await withExaKey("test-key", async () => {
    const messages: ServerMessage[] = [];
    const reg = {
      getAvailable: () => [MODEL],
      find: (prov: string, mid: string) =>
        prov === MODEL.provider && mid === MODEL.id ? MODEL : undefined,
    } as unknown as ExtensionContext["modelRegistry"];
    const gw = new GatewayConnection(reg, {
      write: async (msg) => {
        messages.push(msg);
      },
    });
    gw.handle({
      version: 1,
      type: "web_search",
      id: "ws-net-err",
      token: TOKEN,
      provider: MODEL.provider,
      modelId: MODEL.id,
      query: "test",
    });
    await settle();
    expect(messages).toHaveLength(1);
    expect(messages[0]?.type).toBe("web_search_error");
    expect((messages[0] as Record<string, unknown>)["message"]).toContain("network failure");
  });
});

it("returns error when EXA_API_KEY is empty string", async () => {
  const messages: ServerMessage[] = [];
  const reg = {
    getAvailable: () => [MODEL],
    find: (prov: string, mid: string) =>
      prov === MODEL.provider && mid === MODEL.id ? MODEL : undefined,
  } as unknown as ExtensionContext["modelRegistry"];
  const gw = new GatewayConnection(reg, {
    write: async (msg) => {
      messages.push(msg);
    },
  });
  const origKey = process.env["EXA_API_KEY"];
  process.env["EXA_API_KEY"] = "";
  try {
    gw.handle({
      version: 1,
      type: "web_search",
      id: "ws-empty",
      token: TOKEN,
      provider: MODEL.provider,
      modelId: MODEL.id,
      query: "test",
    });
    await settle();
    expect(messages).toHaveLength(1);
    expect(messages[0]?.type).toBe("web_search_error");
    expect((messages[0] as Record<string, unknown>)["message"]).toContain("EXA_API_KEY");
  } finally {
    if (origKey === undefined) {
      delete process.env["EXA_API_KEY"];
    } else {
      process.env["EXA_API_KEY"] = origKey;
    }
  }
});

it("returns empty results when Exa API returns no results", async () => {
  globalThis.fetch = vi.fn().mockResolvedValue({
    ok: true,
    json: async () => ({ results: [] }),
  });
  await withExaKey("test-key", async () => {
    const messages: ServerMessage[] = [];
    const reg = {
      getAvailable: () => [MODEL],
      find: (prov: string, mid: string) =>
        prov === MODEL.provider && mid === MODEL.id ? MODEL : undefined,
    } as unknown as ExtensionContext["modelRegistry"];
    const gw = new GatewayConnection(reg, {
      write: async (msg) => {
        messages.push(msg);
      },
    });
    gw.handle({
      version: 1,
      type: "web_search",
      id: "ws-empty-results",
      token: TOKEN,
      provider: MODEL.provider,
      modelId: MODEL.id,
      query: "obscure query xyz",
    });
    await settle();
    expect(messages).toHaveLength(1);
    expect(messages[0]?.type).toBe("web_search_result");
    const res = messages[0] as Record<string, unknown>;
    expect(res["results"]).toEqual([]);
  });
});

it("handles Exa results with missing fields", async () => {
  globalThis.fetch = vi.fn().mockResolvedValue({
    ok: true,
    json: async () => ({ results: [{ url: "https://notitle.com" }] }),
  });
  await withExaKey("test-key", async () => {
    const messages: ServerMessage[] = [];
    const reg = {
      getAvailable: () => [MODEL],
      find: (prov: string, mid: string) =>
        prov === MODEL.provider && mid === MODEL.id ? MODEL : undefined,
    } as unknown as ExtensionContext["modelRegistry"];
    const gw = new GatewayConnection(reg, {
      write: async (msg) => {
        messages.push(msg);
      },
    });
    gw.handle({
      version: 1,
      type: "web_search",
      id: "ws-partial",
      token: TOKEN,
      provider: MODEL.provider,
      modelId: MODEL.id,
      query: "test",
    });
    await settle();
    expect(messages).toHaveLength(1);
    const res = messages[0] as Record<string, unknown>;
    const items = res["results"] as { title: string; url: string; snippet: string }[];
    expect(items[0]?.title).toBe("");
    expect(items[0]?.url).toBe("https://notitle.com");
    expect(items[0]?.snippet).toBe("");
  });
});

it("handles Exa response with missing results field", async () => {
  globalThis.fetch = vi.fn().mockResolvedValue({
    ok: true,
    json: async () => ({}),
  });
  await withExaKey("test-key", async () => {
    const messages: ServerMessage[] = [];
    const reg = {
      getAvailable: () => [MODEL],
      find: (prov: string, mid: string) =>
        prov === MODEL.provider && mid === MODEL.id ? MODEL : undefined,
    } as unknown as ExtensionContext["modelRegistry"];
    const gw = new GatewayConnection(reg, {
      write: async (msg) => {
        messages.push(msg);
      },
    });
    gw.handle({
      version: 1,
      type: "web_search",
      id: "ws-no-results-field",
      token: TOKEN,
      provider: MODEL.provider,
      modelId: MODEL.id,
      query: "test",
    });
    await settle();
    expect(messages).toHaveLength(1);
    expect(messages[0]?.type).toBe("web_search_result");
    expect((messages[0] as Record<string, unknown>)["results"]).toEqual([]);
  });
});
