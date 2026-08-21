// This file runs with Bun.

import type { ExtensionAPI, ProviderConfig } from "@earendil-works/pi-coding-agent";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_API_BASE, ENV_API_KEY, PROVIDER_NAME } from "../src/env.ts";

const discoverMock = vi.hoisted(() => vi.fn());
const oauthLoginMock = vi.hoisted(() =>
  vi.fn(async () => ({ access: "access", expires: 0, refresh: "refresh" })),
);
const oauthRefreshTokenMock = vi.hoisted(() =>
  vi.fn(async () => ({ access: "refreshed", expires: 1, refresh: "refresh" })),
);
vi.mock("../src/cline-models.ts", () => ({ discoverClinePassModels: discoverMock }));
vi.mock("../src/oauth.ts", () => ({
  login: oauthLoginMock,
  refreshToken: oauthRefreshTokenMock,
}));

interface ExtensionHarness {
  config: ProviderConfig;
  events: string[];
  providerName: string;
}

async function createHarness(invokeHandler = false): Promise<ExtensionHarness> {
  let config: ProviderConfig | undefined;
  let providerName: string | undefined;
  const events: string[] = [];
  const pendingHandlers: Promise<void>[] = [];
  const host = {
    on(
      event: string,
      handler: (
        event: { message: unknown },
        context: {
          hasUI: boolean;
          model?: { provider: string };
          ui: { notify: () => void };
        },
      ) => void | Promise<void>,
    ): void {
      events.push(event);
      if (invokeHandler) {
        const firstHandler = handler(
          { message: "invalid" },
          { hasUI: true, ui: { notify: () => undefined } },
        );
        if (firstHandler instanceof Promise) {
          pendingHandlers.push(firstHandler);
        }
        const secondHandler = handler(
          { message: "invalid" },
          {
            hasUI: true,
            model: { provider: PROVIDER_NAME },
            ui: { notify: () => undefined },
          },
        );
        if (secondHandler instanceof Promise) {
          pendingHandlers.push(secondHandler);
        }
      }
    },
    registerProvider(name: string, value: ProviderConfig): void {
      providerName = name;
      config = value;
    },
  };
  const { default: clinePassExtension } = await import("../index.ts");
  clinePassExtension(host as unknown as ExtensionAPI);
  if (config === undefined || providerName === undefined) {
    throw new Error("ClinePass provider was not registered");
  }
  await Promise.all(pendingHandlers);
  return { config, events, providerName };
}

function requiredRefresh(config: ProviderConfig): NonNullable<ProviderConfig["refreshModels"]> {
  if (config.refreshModels === undefined) {
    throw new Error("refreshModels was not registered");
  }
  return config.refreshModels;
}

beforeEach(() => {
  discoverMock.mockResolvedValue([
    { modelId: "cline-pass/glm-5.3", name: "cline-pass/glm-5.3" },
    { modelId: "cline-pass/qwen3.8-max", name: "Qwen3.8 Max" },
  ]);
});

afterEach(() => {
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
});

describe("ClinePass provider registration", () => {
  it("registers static models without invoking the Cline CLI", async () => {
    vi.stubEnv(ENV_API_KEY, "");
    const harness = await createHarness();
    const { oauth } = harness.config;

    expect(harness.providerName).toBe(PROVIDER_NAME);
    expect(harness.config.baseUrl).toBe(`${DEFAULT_API_BASE}/api/v1`);
    expect(harness.config.api).toBe("openai-completions");
    expect(harness.config.authHeader).toBe(true);
    expect(harness.config.apiKey).toBeUndefined();
    expect(harness.config.models?.map((model) => model.id)).toStrictEqual([
      "cline-pass/glm-5.3",
      "cline-pass/glm-5.2",
      "cline-pass/kimi-k2.7-code",
      "cline-pass/kimi-k2.6",
      "cline-pass/kimi-k3",
      "cline-pass/deepseek-v4-pro",
      "cline-pass/deepseek-v4-flash",
      "cline-pass/mimo-v2.5",
      "cline-pass/mimo-v2.5-pro",
      "cline-pass/minimax-m3",
      "cline-pass/qwen3.8-max",
      "cline-pass/qwen3.7-max",
      "cline-pass/qwen3.7-plus",
    ]);
    expect(discoverMock).not.toHaveBeenCalled();
    expect(oauth?.name).toBe("ClinePass");
    expect(oauth?.isSubscription).toBe(true);
    expect(harness.events).toStrictEqual(["message_end"]);
  });

  it("does not invoke the Cline CLI during cache-only refresh", async () => {
    const harness = await createHarness();
    const refreshed = await requiredRefresh(harness.config)({
      allowNetwork: false,
      publish: vi.fn(async () => true),
      signal: new AbortController().signal,
    });

    expect(refreshed.map((model) => model.id)).toStrictEqual([
      "cline-pass/glm-5.3",
      "cline-pass/glm-5.2",
      "cline-pass/kimi-k2.7-code",
      "cline-pass/kimi-k2.6",
      "cline-pass/kimi-k3",
      "cline-pass/deepseek-v4-pro",
      "cline-pass/deepseek-v4-flash",
      "cline-pass/mimo-v2.5",
      "cline-pass/mimo-v2.5-pro",
      "cline-pass/minimax-m3",
      "cline-pass/qwen3.8-max",
      "cline-pass/qwen3.7-max",
      "cline-pass/qwen3.7-plus",
    ]);
    expect(discoverMock).not.toHaveBeenCalled();
  });

  it("restores the cached catalog during cache-only refresh", async () => {
    const harness = await createHarness();
    const refreshed = await requiredRefresh(harness.config)({
      allowNetwork: false,
      publish: vi.fn(async () => true),
      signal: new AbortController().signal,
      stored: {
        models: [
          {
            api: "openai-completions",
            baseUrl: DEFAULT_API_BASE,
            contextWindow: 128_000,
            cost: { cacheRead: 0, cacheWrite: 0, input: 0, output: 0 },
            id: "cached-model",
            input: ["text"],
            maxTokens: 8192,
            name: "Cached model",
            provider: PROVIDER_NAME,
            reasoning: false,
          },
        ],
      },
    });

    expect(refreshed.map((model) => model.id)).toStrictEqual(["cached-model"]);
    expect(discoverMock).not.toHaveBeenCalled();
  });

  it("registers the environment API-key reference only when configured", async () => {
    vi.stubEnv(ENV_API_KEY, "test-key");
    expect((await createHarness()).config.apiKey).toBe(`$${ENV_API_KEY}`);
  });

  it("defers OAuth implementation until it is used", async () => {
    const harness = await createHarness();
    const oauth = harness.config.oauth;
    if (oauth === undefined) {
      throw new Error("ClinePass OAuth configuration was not registered");
    }

    const credentials = await oauth.login({
      onAuth: vi.fn(),
      onDeviceCode: vi.fn(),
      onPrompt: vi.fn(async () => "api-key"),
      onSelect: vi.fn(async () => undefined),
    });
    const refreshed = await oauth.refreshToken(credentials, new AbortController().signal);

    expect(credentials).toStrictEqual({ access: "access", expires: 0, refresh: "refresh" });
    expect(refreshed).toStrictEqual({ access: "refreshed", expires: 1, refresh: "refresh" });
    expect(oauth.getApiKey(credentials)).toBe("access");
    expect(oauthLoginMock).toHaveBeenCalledTimes(1);
    expect(oauthRefreshTokenMock).toHaveBeenCalledTimes(1);
  });

  it("refreshes models through the Cline CLI", async () => {
    const harness = await createHarness();
    discoverMock.mockResolvedValueOnce([{ modelId: "cline-pass/new-model", name: "New Model" }]);
    const refreshed = await requiredRefresh(harness.config)({
      allowNetwork: true,
      publish: vi.fn(async () => true),
      signal: new AbortController().signal,
    });

    expect(refreshed.map((model) => model.id)).toStrictEqual(["cline-pass/new-model"]);
    expect(discoverMock).toHaveBeenCalledTimes(1);
  });

  it("wires the error handler for contexts with and without a model", async () => {
    await expect(createHarness(true)).resolves.toMatchObject({ events: ["message_end"] });
  });

  it("propagates Cline CLI discovery failures during refresh", async () => {
    const harness = await createHarness();
    discoverMock.mockRejectedValueOnce(new Error("cline command not found"));
    await expect(
      requiredRefresh(harness.config)({
        allowNetwork: true,
        publish: vi.fn(async () => true),
        signal: new AbortController().signal,
      }),
    ).rejects.toThrow("cline command not found");
  });
});
