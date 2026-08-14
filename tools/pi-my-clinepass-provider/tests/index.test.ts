import type { ExtensionAPI, ProviderConfig } from "@earendil-works/pi-coding-agent";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_API_BASE, ENV_API_KEY, PROVIDER_NAME } from "../src/env.ts";

const discoverMock = vi.hoisted(() => vi.fn());
vi.mock("../src/cline-models.ts", () => ({ discoverClinePassModels: discoverMock }));

interface ExtensionHarness {
  config: ProviderConfig;
  events: string[];
  providerName: string;
}

async function createHarness(invokeHandler = false): Promise<ExtensionHarness> {
  let config: ProviderConfig | undefined;
  let providerName: string | undefined;
  const events: string[] = [];
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
      ) => void,
    ): void {
      events.push(event);
      if (invokeHandler) {
        handler({ message: "invalid" }, { hasUI: true, ui: { notify: () => undefined } });
        handler(
          { message: "invalid" },
          {
            hasUI: true,
            model: { provider: PROVIDER_NAME },
            ui: { notify: () => undefined },
          },
        );
      }
    },
    registerProvider(name: string, value: ProviderConfig): void {
      providerName = name;
      config = value;
    },
  };
  const { default: clinePassExtension } = await import("../index.ts");
  await clinePassExtension(host as unknown as ExtensionAPI);
  if (config === undefined || providerName === undefined) {
    throw new Error("ClinePass provider was not registered");
  }
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
  it("registers models discovered from the Cline CLI", async () => {
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
      "cline-pass/qwen3.8-max",
    ]);
    expect(oauth?.name).toBe("ClinePass");
    expect(oauth?.isSubscription).toBe(true);
    expect(harness.events).toStrictEqual(["message_end"]);
  });

  it("registers the environment API-key reference only when configured", async () => {
    vi.stubEnv(ENV_API_KEY, "test-key");
    expect((await createHarness()).config.apiKey).toBe(`$${ENV_API_KEY}`);
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
    expect(discoverMock).toHaveBeenCalledTimes(2);
  });

  it("wires the error handler for contexts with and without a model", async () => {
    await expect(createHarness(true)).resolves.toMatchObject({ events: ["message_end"] });
  });

  it("propagates Cline CLI discovery failures", async () => {
    discoverMock.mockRejectedValueOnce(new Error("cline command not found"));
    await expect(createHarness()).rejects.toThrow("cline command not found");
  });
});
