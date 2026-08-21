// This file runs with Bun.

import process from "node:process";
import type {
  Api,
  Model,
  OAuthCredentials,
  OAuthLoginCallbacks,
  RefreshModelsContext,
} from "@earendil-works/pi-ai";
import type { ExtensionAPI, ProviderModelConfig } from "@earendil-works/pi-coding-agent";
import { resolveApiBase, ENV_API_KEY, PROVIDER_NAME } from "./src/env.ts";
import { MODELS, modelsFromClineCatalog, type ModelConfig } from "./src/models.ts";

function toProviderModels(models: readonly ModelConfig[]): ProviderModelConfig[] {
  return models.map((model) => ({
    ...model,
    input: [...model.input],
  }));
}

function cloneProviderModel(model: Model<Api>): ProviderModelConfig {
  return { ...model, input: [...model.input] };
}

const FALLBACK_MODELS: ProviderModelConfig[] = toProviderModels(MODELS);

function restoreModels(context: RefreshModelsContext): ProviderModelConfig[] {
  const stored = context.stored?.models;
  return stored === undefined || stored.length === 0
    ? FALLBACK_MODELS
    : stored.map((model) => cloneProviderModel(model));
}

async function loadModels(context: RefreshModelsContext): Promise<ProviderModelConfig[]> {
  if (!context.allowNetwork) {
    return restoreModels(context);
  }
  const { discoverClinePassModels } = await import("./src/cline-models.ts");
  const catalog = await discoverClinePassModels();
  return toProviderModels(modelsFromClineCatalog(catalog));
}

async function login(callbacks: OAuthLoginCallbacks): Promise<OAuthCredentials> {
  const module = await import("./src/oauth.ts");
  return module.login(callbacks);
}

async function refreshToken(credentials: OAuthCredentials): Promise<OAuthCredentials> {
  const module = await import("./src/oauth.ts");
  return module.refreshToken(credentials);
}

function getApiKey(credentials: OAuthCredentials): string {
  return credentials.access;
}

export default function clinePassExtension(pi: ExtensionAPI): void {
  const apiBase = resolveApiBase();
  const envApiKey = process.env[ENV_API_KEY]?.trim();

  pi.registerProvider(PROVIDER_NAME, {
    name: "ClinePass",
    baseUrl: `${apiBase}/api/v1`,
    ...(envApiKey === undefined || envApiKey === "" ? {} : { apiKey: `$${ENV_API_KEY}` }),
    api: "openai-completions",
    authHeader: true,
    // Keep startup local; refreshModels performs the optional Cline ACP lookup on demand.
    models: FALLBACK_MODELS,
    refreshModels: loadModels,
    oauth: {
      name: "ClinePass",
      isSubscription: true,
      login,
      refreshToken,
      getApiKey,
    },
  });

  pi.on("message_end", async (event, context) => {
    const { handleClinePassError } = await import("./src/error-handler.ts");
    handleClinePassError(event, {
      hasUI: context.hasUI,
      ...(context.model === undefined ? {} : { model: context.model }),
      ui: context.ui,
    });
  });
}
