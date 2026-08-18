import { mkdtemp, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { loadClaudexModels } from "../src/claudex-models.ts";
import {
  CLAUDEX_AUTH_ENV,
  CLAUDEX_BASE_URL_ENV,
  CLAUDEX_CONFIG_ENV,
  CLAUDEX_ORIGIN_HEADER,
  CLAUDEX_ORIGIN_VALUE,
  createClaudexProviderConfig,
} from "../src/claudex-provider.ts";

const roots: string[] = [];

async function configFile(value: unknown): Promise<string> {
  const root = await mkdtemp("/tmp/pi-claudex-models-");
  roots.push(root);
  const file = path.join(root, "providers.json");
  await writeFile(file, JSON.stringify(value));
  return file;
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
    const models = await loadClaudexModels(file);
    expect(
      models.map((model) => ({ id: model.id, contextWindow: model.contextWindow })),
    ).toStrictEqual([
      { id: "gpt-5.6-luna", contextWindow: 110_000 },
      { id: "gpt-5.6-terra", contextWindow: 110_000 },
      { id: "glm-5.2:cloud", contextWindow: 200_000 },
      { id: "claude-sonnet-5", contextWindow: 200_000 },
      { id: "claude-opus-5", contextWindow: 200_000 },
    ]);
    expect(models[0]).toMatchObject({
      name: "Claudex · gpt-5.6-luna",
      reasoning: true,
      input: ["text", "image"],
      maxTokens: 32_768,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    });
  });

  it("rejects non-object and empty catalogs", async () => {
    await expect(loadClaudexModels(await configFile([]))).rejects.toThrow("must be a JSON object");
    await expect(loadClaudexModels(await configFile({ providers: [] }))).rejects.toThrow(
      "contains no enabled models",
    );
  });

  it("honors an abort signal during model loading", async () => {
    const file = await configFile({ providers: [{ defaultModel: "gpt-5.6-luna" }] });
    const controller = new AbortController();
    controller.abort();
    await expect(loadClaudexModels(file, controller.signal)).rejects.toThrow(
      "Claudex model refresh was aborted",
    );
  });
});

describe("Claudex Pi provider registration", () => {
  it("uses the loopback adapter, origin marker, and built-in Anthropic streaming", async () => {
    const file = await configFile({ providers: [{ defaultModel: "gpt-5.6-luna" }] });
    const config = await createClaudexProviderConfig({ [CLAUDEX_CONFIG_ENV]: file });
    expect(config).toMatchObject({
      name: "Claudex",
      baseUrl: "http://127.0.0.1:8318",
      apiKey: "claudex-loopback",
      api: "anthropic-messages",
      headers: { [CLAUDEX_ORIGIN_HEADER]: CLAUDEX_ORIGIN_VALUE },
    });
    expect(config.models?.map((model) => model.id)).toStrictEqual(["gpt-5.6-luna"]);
  });

  it("uses configured HTTPS, config, and auth environment references", async () => {
    const file = await configFile({ providers: [{ defaultModel: "model-a" }] });
    const config = await createClaudexProviderConfig({
      [CLAUDEX_CONFIG_ENV]: file,
      [CLAUDEX_BASE_URL_ENV]: "https://adapter.example.test/",
      [CLAUDEX_AUTH_ENV]: "secret",
    });
    expect(config.baseUrl).toBe("https://adapter.example.test");
    expect(config.apiKey).toBe(`$${CLAUDEX_AUTH_ENV}`);
    await writeFile(file, JSON.stringify({ providers: [{ defaultModel: "model-b" }] }));
    const refreshed = await config.refreshModels?.({
      allowNetwork: false,
      publish: async () => true,
      signal: new AbortController().signal,
    });
    expect(refreshed?.map((model) => model.id)).toStrictEqual(["model-b"]);
  });

  it("propagates an abort signal to model refresh", async () => {
    const file = await configFile({ providers: [{ defaultModel: "model-a" }] });
    const config = await createClaudexProviderConfig({ [CLAUDEX_CONFIG_ENV]: file });
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
