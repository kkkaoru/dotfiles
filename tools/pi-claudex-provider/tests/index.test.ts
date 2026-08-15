import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { beforeEach, describe, expect, it, vi } from "vitest";

const createProviderConfigMock = vi.hoisted(() => vi.fn());
const resolveConfigMock = vi.hoisted(() => vi.fn());
const startServerMock = vi.hoisted(() => vi.fn());
vi.mock("../src/claudex-provider.ts", () => ({
  CLAUDEX_PROVIDER_ID: "claudex",
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
    expect([...test.handlers.keys()]).toStrictEqual([]);
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
    expect([...test.handlers.keys()]).toStrictEqual(["session_start", "session_shutdown"]);
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
