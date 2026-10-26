import type { ProviderConfig } from "@earendil-works/pi-coding-agent";
import type { RefreshModelsContext } from "@earendil-works/pi-ai";
import { loadClaudexModels } from "./claudex-models.ts";
import { isRecord } from "./protocol.ts";
import {
  cachedSubscriptionModels,
  type ListSupportedModels,
  refreshSubscriptionModels,
  SUBSCRIPTION_MODELS_CACHE,
  sdkSupportedModels,
} from "./subscription-models.ts";

export const CLAUDEX_PROVIDER_ID = "claudex";
export const CLAUDEX_ORIGIN_HEADER = "x-claudex-origin";
export const CLAUDEX_ORIGIN_VALUE = "pi-provider";
export const CLAUDEX_BASE_URL_ENV = "CLAUDEX_ADAPTER_BASE_URL";
export const CLAUDEX_CONFIG_ENV = "CLAUDEX_PROVIDER_CONFIG";
export const CLAUDEX_AUTH_ENV = "ANTHROPIC_AUTH_TOKEN";
export const CLAUDEX_WORKING_DIRECTORY_HEADER = "x-claudex-working-directory";
const DEFAULT_BASE_URL = "http://127.0.0.1:8318";
const DEFAULT_CONFIG_PATH = "~/.config/claudex/providers.json";
const LOOPBACK_API_KEY = "claudex-local";

interface ClaudexProviderSettings {
  baseUrl: string;
  configPath: string;
  apiKey: string;
}

function isLoopback(hostname: string): boolean {
  return hostname === "127.0.0.1" || hostname === "::1" || hostname === "localhost";
}

function nonEmptyEnv(
  env: Readonly<Record<string, string | undefined>>,
  key: string,
): string | undefined {
  const value = env[key]?.trim();
  return value === undefined || value === "" ? undefined : value;
}

function adapterUrl(value: string): URL {
  const url = new URL(value);
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error(`${CLAUDEX_BASE_URL_ENV} must use http or https`);
  }
  return url;
}

function resolveSettings(
  env: Readonly<Record<string, string | undefined>>,
): ClaudexProviderSettings {
  const url = adapterUrl(nonEmptyEnv(env, CLAUDEX_BASE_URL_ENV) ?? DEFAULT_BASE_URL);
  const token = nonEmptyEnv(env, CLAUDEX_AUTH_ENV);
  if (!isLoopback(url.hostname) && token === undefined) {
    throw new Error(`${CLAUDEX_AUTH_ENV} is required for a non-loopback Claudex adapter`);
  }
  return {
    baseUrl: url.toString().replace(/\/$/u, ""),
    configPath: nonEmptyEnv(env, CLAUDEX_CONFIG_ENV) ?? DEFAULT_CONFIG_PATH,
    apiKey: token === undefined ? LOOPBACK_API_KEY : `$${CLAUDEX_AUTH_ENV}`,
  };
}

export const CLAUDEX_SUBSCRIPTION_CACHE_ENV = "CLAUDEX_SUBSCRIPTION_MODELS_CACHE";

/* Startup reads the cached plan model list; pi's background catalog refresh asks the Agent SDK
   for the live list and updates the cache. */
export async function createClaudexProviderConfig(
  env: Readonly<Record<string, string | undefined>> = process.env,
  listSupportedModels: ListSupportedModels = sdkSupportedModels,
): Promise<ProviderConfig> {
  const settings = resolveSettings(env);
  const cachePath = nonEmptyEnv(env, CLAUDEX_SUBSCRIPTION_CACHE_ENV) ?? SUBSCRIPTION_MODELS_CACHE;
  const models = await loadClaudexModels(settings.configPath, {
    subscription: await cachedSubscriptionModels(cachePath),
  });
  return {
    name: "Claudex",
    baseUrl: settings.baseUrl,
    apiKey: settings.apiKey,
    api: "anthropic-messages",
    headers: { [CLAUDEX_ORIGIN_HEADER]: CLAUDEX_ORIGIN_VALUE },
    models,
    refreshModels: async (context?: RefreshModelsContext) =>
      loadClaudexModels(settings.configPath, {
        signal: context?.signal,
        subscription: await refreshSubscriptionModels({
          list: listSupportedModels,
          cachePath,
          signal: context?.signal,
        }),
      }),
  };
}

/** Claudex runs Claude Code with its own system prompt, so pi's is not sent. */
export function withoutSystemPrompt(payload: unknown): unknown {
  if (!isRecord(payload) || !("system" in payload)) {
    return undefined;
  }
  const { system: _system, ...rest } = payload;
  return rest;
}

/** The adapter otherwise reads the working directory from the dropped system prompt. */
export function workingDirectoryHeader(cwd: string): string {
  return encodeURIComponent(cwd);
}
