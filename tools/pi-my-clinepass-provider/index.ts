import process from "node:process";
import type { ExtensionAPI, ProviderModelConfig } from "@earendil-works/pi-coding-agent";
import { discoverClinePassModels } from "./src/cline-models.ts";
import { resolveApiBase, ENV_API_KEY, PROVIDER_NAME } from "./src/env.ts";
import { handleClinePassError } from "./src/error-handler.ts";
import { modelsFromClineCatalog, type ModelConfig } from "./src/models.ts";
import { getApiKey, login, refreshToken } from "./src/oauth.ts";

function toProviderModels(models: readonly ModelConfig[]): ProviderModelConfig[] {
  return models.map((model) => ({
    ...model,
    input: [...model.input],
  }));
}

async function loadModels(): Promise<ProviderModelConfig[]> {
  const catalog = await discoverClinePassModels();
  return toProviderModels(modelsFromClineCatalog(catalog));
}

export default async function clinePassExtension(pi: ExtensionAPI): Promise<void> {
  const apiBase = resolveApiBase();
  const envApiKey = process.env[ENV_API_KEY]?.trim();
  const models = await loadModels();

  pi.registerProvider(PROVIDER_NAME, {
    name: "ClinePass",
    baseUrl: `${apiBase}/api/v1`,
    ...(envApiKey === undefined || envApiKey === "" ? {} : { apiKey: `$${ENV_API_KEY}` }),
    api: "openai-completions",
    authHeader: true,
    models,
    refreshModels: loadModels,
    oauth: {
      name: "ClinePass",
      isSubscription: true,
      login,
      refreshToken,
      getApiKey,
    },
  });

  pi.on("message_end", (event, context) => {
    handleClinePassError(event, {
      hasUI: context.hasUI,
      ...(context.model === undefined ? {} : { model: context.model }),
      ui: context.ui,
    });
  });
}
