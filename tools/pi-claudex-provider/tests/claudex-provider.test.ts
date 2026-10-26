import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { loadClaudexModels } from "../src/claudex-models.ts";
import {
  CLAUDEX_AUTH_ENV,
  CLAUDEX_BASE_URL_ENV,
  CLAUDEX_CONFIG_ENV,
  CLAUDEX_ORIGIN_HEADER,
  CLAUDEX_ORIGIN_VALUE,
  CLAUDEX_SUBSCRIPTION_CACHE_ENV,
  createClaudexProviderConfig,
  withoutSystemPrompt,
} from "../src/claudex-provider.ts";

const HAIKU_PLAN = [{ id: "claude-haiku-5-5", contextWindow: 1_000_000 }];

const roots: string[] = [];

async function configFile(value: unknown): Promise<string> {
  const root = await mkdtemp("/tmp/pi-claudex-models-");
  roots.push(root);
  const file = path.join(root, "providers.json");
  await writeFile(file, JSON.stringify(value));
  return file;
}

async function cacheFile(ids: readonly string[]): Promise<string> {
  const file = await configFile(ids);
  return file;
}

async function noLiveModels(): Promise<never[]> {
  return [];
}

afterEach(async () => {
  await Promise.all(
    roots.splice(0).map(async (root) => {
      await rm(root, { recursive: true, force: true });
    }),
  );
});

describe("Claudex model catalog", () => {
  it("collects unique enabled provider, selectable, native, fallback, and advisor models", async () => {
    const file = await configFile({
      providers: [
        {
          enabled: true,
          defaultModel: "gpt-5.6-luna",
          subagentModel: "gpt-5.6-luna",
          selectableModels: ["gpt-5.6-terra", 42],
          maxContextTokens: 110_000,
        },
        { enabled: false, defaultModel: "disabled" },
        { enabled: true, defaultModel: "glm-5.2:cloud", maxContextTokens: -1 },
        null,
      ],
      nativeWorkers: [{ model: "claude-sonnet-5" }, "invalid"],
      fallback: { model: "claude-sonnet-5" },
      advisor: { model: "claude-opus-5" },
    });
    const models = await loadClaudexModels(file, { subscription: HAIKU_PLAN });
    expect(
      models.map((model) => ({
        id: model.id,
        contextWindow: "contextWindow" in model ? model.contextWindow : undefined,
      })),
    ).toStrictEqual([
      { id: "gpt-5.6-luna", contextWindow: 110_000 },
      { id: "gpt-5.6-terra", contextWindow: 110_000 },
      { id: "glm-5.2:cloud", contextWindow: 200_000 },
      { id: "claude-sonnet-5", contextWindow: 200_000 },
      { id: "claude-opus-5", contextWindow: 200_000 },
      { id: "claude-haiku-5-5", contextWindow: 1_000_000 },
    ]);
    expect(models[0]).toMatchObject({
      name: "Claudex · gpt-5.6-luna",
      reasoning: true,
      compat: { forceAdaptiveThinking: true },
      thinkingLevelMap: {
        off: null,
        minimal: "minimal",
        low: "low",
        medium: "medium",
        high: "high",
        xhigh: "xhigh",
        max: "max",
      },
      input: ["text", "image"],
      maxTokens: 32_768,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    });
  });

  it("rejects non-object and empty catalogs", async () => {
    await expect(loadClaudexModels(await configFile([]), { subscription: [] })).rejects.toThrow(
      "must be a JSON object",
    );
    await expect(
      loadClaudexModels(await configFile({ providers: [] }), { subscription: [] }),
    ).rejects.toThrow("contains no enabled models");
  });

  it("loads a catalog that contains only a fallback model", async () => {
    const file = await configFile({ fallback: { model: "fallback-only" } });
    const models = await loadClaudexModels(file, { subscription: HAIKU_PLAN });
    expect(models.map((model) => model.id)).toStrictEqual(["fallback-only", "claude-haiku-5-5"]);
  });

  it("keeps a catalog entry for a built-in subscription model instead of duplicating it", async () => {
    const file = await configFile({
      fallback: { model: "claude-haiku-5-5", maxContextTokens: 500_000 },
    });
    const models = await loadClaudexModels(file, { subscription: HAIKU_PLAN });
    expect(
      models.map((model) => ({
        id: model.id,
        contextWindow: "contextWindow" in model ? model.contextWindow : undefined,
      })),
    ).toStrictEqual([{ id: "claude-haiku-5-5", contextWindow: 500_000 }]);
  });

  it("honors an abort signal during model loading", async () => {
    const file = await configFile({ providers: [{ defaultModel: "gpt-5.6-luna" }] });
    const controller = new AbortController();
    controller.abort();
    await expect(
      loadClaudexModels(file, { signal: controller.signal, subscription: [] }),
    ).rejects.toThrow("Claudex model refresh was aborted");
  });
});

describe("Claudex Pi provider registration", () => {
  it("uses the loopback adapter, origin marker, and built-in Anthropic streaming", async () => {
    const file = await configFile({ providers: [{ defaultModel: "gpt-5.6-luna" }] });
    const config = await createClaudexProviderConfig(
      {
        [CLAUDEX_CONFIG_ENV]: file,
        [CLAUDEX_SUBSCRIPTION_CACHE_ENV]: await cacheFile(["claude-haiku-5-5"]),
      },
      noLiveModels,
    );
    expect(config).toMatchObject({
      name: "Claudex",
      baseUrl: "http://127.0.0.1:8318",
      apiKey: "claudex-local",
      api: "anthropic-messages",
      headers: { [CLAUDEX_ORIGIN_HEADER]: CLAUDEX_ORIGIN_VALUE },
    });
    expect(config.models?.map((model) => model.id)).toStrictEqual([
      "gpt-5.6-luna",
      "claude-haiku-5-5",
    ]);
  });

  it("uses configured HTTPS, config, and auth environment references", async () => {
    const file = await configFile({ providers: [{ defaultModel: "model-a" }] });
    const cache = await cacheFile(["claude-haiku-5-5"]);
    const config = await createClaudexProviderConfig(
      {
        [CLAUDEX_CONFIG_ENV]: file,
        [CLAUDEX_BASE_URL_ENV]: "https://adapter.example.test/",
        [CLAUDEX_AUTH_ENV]: "secret",
        [CLAUDEX_SUBSCRIPTION_CACHE_ENV]: cache,
      },
      async () => [
        { value: "default", resolvedModel: "claude-opus-5-5" },
        { value: "opus", resolvedModel: "claude-opus-5-5" },
        { value: "claude-sonnet-4-6" },
      ],
    );
    expect(config.baseUrl).toBe("https://adapter.example.test");
    expect(config.apiKey).toBe(`$${CLAUDEX_AUTH_ENV}`);
    await writeFile(file, JSON.stringify({ providers: [{ defaultModel: "model-b" }] }));
    const refreshed = await config.refreshModels?.({
      allowNetwork: false,
      publish: async () => true,
      signal: new AbortController().signal,
    });
    expect(refreshed?.map((model) => model.id)).toStrictEqual([
      "model-b",
      "claude-opus-5-5",
      "claude-sonnet-4-6",
    ]);
    expect(await readFile(cache, "utf8")).toBe('["claude-opus-5-5","claude-sonnet-4-6"]\n');
  });

  it("propagates an abort signal to model refresh", async () => {
    const file = await configFile({ providers: [{ defaultModel: "model-a" }] });
    const config = await createClaudexProviderConfig(
      {
        [CLAUDEX_CONFIG_ENV]: file,
        [CLAUDEX_SUBSCRIPTION_CACHE_ENV]: await cacheFile(["claude-haiku-5-5"]),
      },
      noLiveModels,
    );
    const controller = new AbortController();
    controller.abort();
    await expect(
      config.refreshModels?.({
        allowNetwork: false,
        publish: async () => true,
        signal: controller.signal,
      }),
    ).rejects.toThrow("Claudex model refresh was aborted");
  });

  it("rejects invalid protocols and unauthenticated non-loopback adapters", async () => {
    const file = await configFile({ providers: [{ defaultModel: "model" }] });
    await expect(
      createClaudexProviderConfig({
        [CLAUDEX_CONFIG_ENV]: file,
        [CLAUDEX_BASE_URL_ENV]: "file:///tmp/adapter",
      }),
    ).rejects.toThrow("must use http or https");
    await expect(
      createClaudexProviderConfig({
        [CLAUDEX_CONFIG_ENV]: file,
        [CLAUDEX_BASE_URL_ENV]: "https://adapter.example.test",
      }),
    ).rejects.toThrow(`${CLAUDEX_AUTH_ENV} is required`);
  });
});

it("leaves payloads without a system prompt untouched", () => {
  expect(withoutSystemPrompt({ messages: [] })).toBe(undefined);
  expect(withoutSystemPrompt("raw")).toBe(undefined);
  expect(withoutSystemPrompt({ system: "pi", messages: [] })).toStrictEqual({ messages: [] });
});
