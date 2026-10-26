import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { beforeEach, describe, expect, it, vi } from "vitest";

const createProviderConfigMock = vi.hoisted(() => vi.fn());
const resolveConfigMock = vi.hoisted(() => vi.fn());
const startServerMock = vi.hoisted(() => vi.fn());
vi.mock("../src/claudex-provider.ts", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../src/claudex-provider.ts")>()),
  createClaudexProviderConfig: createProviderConfigMock,
}));
vi.mock("../src/config.ts", () => ({ resolveGatewayConfig: resolveConfigMock }));
vi.mock("../src/socket-server.ts", () => ({ startGatewayServer: startServerMock }));

interface Harness {
  handlers: Map<string, (...args: unknown[]) => unknown>;
  providers: string[];
  host: ExtensionAPI;
}

function harness(): Harness {
  const handlers = new Map<string, (...args: unknown[]) => unknown>();
  const providers: string[] = [];
  const host = {
    on(event: string, handler: (...args: unknown[]) => unknown): void {
      handlers.set(event, handler);
    },
    registerProvider(name: string): void {
      providers.push(name);
    },
  };
  return { handlers, providers, host: host as unknown as ExtensionAPI };
}

function requiredLifecycle(test: Harness) {
  const start = test.handlers.get("session_start");
  const shutdown = test.handlers.get("session_shutdown");
  if (start === undefined || shutdown === undefined) {
    throw new Error("handlers missing");
  }
  return { start, shutdown };
}

beforeEach(() => {
  createProviderConfigMock.mockReset();
  createProviderConfigMock.mockResolvedValue({ name: "Claudex" });
  resolveConfigMock.mockReset();
  startServerMock.mockReset();
});

describe("pi extension lifecycle", () => {
  it("does not register gateway handlers when configuration is absent", async () => {
    resolveConfigMock.mockReturnValue(undefined);
    const extension = (await import("../index.ts")).default;
    const test = harness();
    await extension(test.host);
    expect(test.providers).toStrictEqual(["claudex"]);
    expect([...test.handlers.keys()]).toStrictEqual([
      "before_provider_request",
      "before_provider_headers",
    ]);
  });

  it("drops pi's system prompt and sends the cwd only for claudex requests", async () => {
    resolveConfigMock.mockReturnValue(undefined);
    const extension = (await import("../index.ts")).default;
    const test = harness();
    await extension(test.host);
    const request = test.handlers.get("before_provider_request");
    const headers = test.handlers.get("before_provider_headers");
    const claudex = { model: { provider: "claudex" }, cwd: "/tmp/a b" };
    const other = { model: { provider: "anthropic" }, cwd: "/tmp/a b" };
    const payload = {
      model: "claude-haiku-5-5",
      system: [{ type: "text", text: "pi" }],
      messages: [],
    };
    expect(request?.({ payload }, claudex)).toStrictEqual({
      model: "claude-haiku-5-5",
      messages: [],
    });
    expect(request?.({ payload }, other)).toBe(undefined);
    const claudexHeaders: Record<string, string> = {};
    headers?.({ headers: claudexHeaders }, claudex);
    expect(claudexHeaders).toStrictEqual({ "x-claudex-working-directory": "%2Ftmp%2Fa%20b" });
    const otherHeaders: Record<string, string> = {};
    headers?.({ headers: otherHeaders }, other);
    expect(otherHeaders).toStrictEqual({});
  });

  it("starts, replaces, and shuts down the configured gateway", async () => {
    const config = { socketPath: "/tmp/test.sock", token: "x".repeat(32) };
    const firstClose = vi.fn(async () => {});
    const secondClose = vi.fn(async () => {});
    resolveConfigMock.mockReturnValue(config);
    startServerMock
      .mockResolvedValueOnce({ close: firstClose })
      .mockResolvedValueOnce({ close: secondClose });
    const extension = (await import("../index.ts")).default;
    const test = harness();
    await extension(test.host);
    expect(test.providers).toStrictEqual(["claudex"]);
    expect([...test.handlers.keys()]).toStrictEqual([
      "before_provider_request",
      "before_provider_headers",
      "session_start",
      "session_shutdown",
    ]);
    const { start, shutdown } = requiredLifecycle(test);
    const context = { modelRegistry: { marker: true } } as unknown as ExtensionContext;
    await start({}, context);
    await start({}, context);
    expect(firstClose).toHaveBeenCalledTimes(1);
    expect(startServerMock).toHaveBeenNthCalledWith(1, config, context.modelRegistry);
    expect(startServerMock).toHaveBeenNthCalledWith(2, config, context.modelRegistry);
    await shutdown({});
    await shutdown({});
    expect(secondClose).toHaveBeenCalledTimes(1);
  });
});
