import { describe, expect, it } from "vitest";
import { GATEWAY_SOCKET_ENV, GATEWAY_TOKEN_ENV, resolveGatewayConfig } from "../src/config.ts";
import { parseClientMessage, serverMessage } from "../src/protocol.ts";

const TOKEN = "01234567890123456789012345678901";

function parse(value: unknown) {
  return parseClientMessage(JSON.stringify(value));
}

function request(overrides: Record<string, unknown> = {}) {
  return {
    version: 1,
    type: "request",
    id: "req-1",
    token: TOKEN,
    origin: "claudex",
    provider: "openai-codex",
    modelId: "gpt-5.6-luna",
    system: null,
    messages: [],
    tools: [],
    options: {},
    ...overrides,
  };
}

describe("gateway config", () => {
  it("leaves the gateway disabled when both variables are absent", () => {
    expect(resolveGatewayConfig({})).toBeUndefined();
  });

  it("accepts a private absolute socket configuration", () => {
    expect(
      resolveGatewayConfig({
        [GATEWAY_SOCKET_ENV]: " /tmp/pi-gateway.sock ",
        [GATEWAY_TOKEN_ENV]: ` ${TOKEN} `,
      }),
    ).toStrictEqual({ socketPath: "/tmp/pi-gateway.sock", token: TOKEN });
  });

  it("rejects partial, relative, long, and weak configurations", () => {
    expect(() => resolveGatewayConfig({ [GATEWAY_SOCKET_ENV]: "/tmp/a.sock" })).toThrow(
      "must be configured together",
    );
    expect(() =>
      resolveGatewayConfig({ [GATEWAY_SOCKET_ENV]: "relative.sock", [GATEWAY_TOKEN_ENV]: TOKEN }),
    ).toThrow("must be an absolute path");
    expect(() =>
      resolveGatewayConfig({
        [GATEWAY_SOCKET_ENV]: `/${"a".repeat(104)}`,
        [GATEWAY_TOKEN_ENV]: TOKEN,
      }),
    ).toThrow("exceeds the Unix socket path limit");
    expect(() =>
      resolveGatewayConfig({ [GATEWAY_SOCKET_ENV]: "/tmp/a.sock", [GATEWAY_TOKEN_ENV]: "short" }),
    ).toThrow("at least 32 characters");
    expect(() =>
      resolveGatewayConfig({ [GATEWAY_SOCKET_ENV]: "", [GATEWAY_TOKEN_ENV]: TOKEN }),
    ).toThrow("must be configured together");
  });
});

describe("gateway protocol", () => {
  it("parses handshake, list, cancel, and complete request options", () => {
    expect(parse({ version: 1, type: "hello", token: TOKEN })).toStrictEqual({
      version: 1,
      type: "hello",
      token: TOKEN,
    });
    expect(parse({ version: 1, type: "list_models", id: "models", token: TOKEN })).toStrictEqual({
      version: 1,
      type: "list_models",
      id: "models",
      token: TOKEN,
    });
    expect(parse({ version: 1, type: "cancel", id: "req-1", token: TOKEN })).toStrictEqual({
      version: 1,
      type: "cancel",
      id: "req-1",
      token: TOKEN,
    });
    expect(
      parse(
        request({
          system: "system",
          messages: [{ role: "user", content: "hi" }],
          tools: [{ name: "clock" }],
          options: {
            reasoning: "high",
            maxTokens: 100,
            temperature: 0.2,
            metadata: { user_id: "u" },
            sessionId: "session",
            cacheRetention: "long",
          },
        }),
      ),
    ).toStrictEqual(
      request({
        system: "system",
        messages: [{ role: "user", content: "hi" }],
        tools: [{ name: "clock" }],
        options: {
          reasoning: "high",
          maxTokens: 100,
          temperature: 0.2,
          metadata: { user_id: "u" },
          sessionId: "session",
          cacheRetention: "long",
        },
      }),
    );
  });

  it("accepts every reasoning and cache-retention family", () => {
    expect(parse(request({ options: { reasoning: "off", cacheRetention: "none" } }))).toMatchObject(
      {
        options: { reasoning: "off", cacheRetention: "none" },
      },
    );
    expect(
      parse(request({ options: { reasoning: "minimal", cacheRetention: "short" } })),
    ).toMatchObject({
      options: { reasoning: "minimal", cacheRetention: "short" },
    });
    expect(parse(request({ options: { reasoning: "low" } }))).toMatchObject({
      options: { reasoning: "low" },
    });
    expect(parse(request({ options: { reasoning: "medium" } }))).toMatchObject({
      options: { reasoning: "medium" },
    });
    expect(parse(request({ options: { reasoning: "xhigh" } }))).toMatchObject({
      options: { reasoning: "xhigh" },
    });
    expect(parse(request({ options: { reasoning: "max" } }))).toMatchObject({
      options: { reasoning: "max" },
    });
  });

  it("builds versioned server messages", () => {
    expect(serverMessage("ready")).toStrictEqual({ version: 1, type: "ready" });
    expect(serverMessage("done", { id: "r" })).toStrictEqual({ version: 1, type: "done", id: "r" });
  });

  it("rejects malformed envelopes and recursion", () => {
    expect(() => parseClientMessage("{")).toThrow("Invalid gateway JSON");
    expect(() => parse([])).toThrow("must be a JSON object");
    expect(() => parse({ version: 2, type: "hello", token: TOKEN })).toThrow("version must be 1");
    expect(() => parse({ version: 1, type: "hello", token: "" })).toThrow("token");
    expect(() => parse({ version: 1, type: "unknown", id: "r", token: TOKEN })).toThrow(
      "Unsupported gateway message type",
    );
    expect(() => parse(request({ origin: "pi" }))).toThrow("origin must be claudex");
    expect(() => parse(request({ provider: "claudex" }))).toThrow("recursion rejected");
    expect(() => parse(request({ id: "x".repeat(257) }))).toThrow("id is too long");
  });

  it("rejects malformed request fields and options", () => {
    expect(() => parse(request({ messages: {} }))).toThrow("messages must be an array");
    expect(() => parse(request({ tools: {} }))).toThrow("tools must be an array");
    expect(() => parse(request({ options: null }))).toThrow("options must be an object");
    expect(() => parse(request({ options: { reasoning: "ultra" } }))).toThrow(
      "reasoning is invalid",
    );
    expect(() => parse(request({ options: { cacheRetention: "forever" } }))).toThrow(
      "cacheRetention is invalid",
    );
    expect(() => parse(request({ options: { maxTokens: 0 } }))).toThrow("positive integer");
    expect(() => parse(request({ options: { temperature: Number.POSITIVE_INFINITY } }))).toThrow(
      "temperature must be finite",
    );
    expect(() => parse(request({ options: { sessionId: "" } }))).toThrow("non-empty string");
    expect(() => parse(request({ options: { metadata: [] } }))).toThrow(
      "metadata must be an object",
    );
  });
});
