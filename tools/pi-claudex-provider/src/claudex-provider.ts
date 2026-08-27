import type { ProviderConfig } from "@earendil-works/pi-coding-agent";
import type { RefreshModelsContext } from "@earendil-works/pi-ai";
import { loadClaudexModels } from "./claudex-models.ts";

export const CLAUDEX_PROVIDER_ID = "claudex";
export const CLAUDEX_ORIGIN_HEADER = "x-claudex-origin";
export const CLAUDEX_ORIGIN_VALUE = "pi-provider";
export const CLAUDEX_BASE_URL_ENV = "CLAUDEX_ADAPTER_BASE_URL";
export const CLAUDEX_CONFIG_ENV = "CLAUDEX_PROVIDER_CONFIG";
export const CLAUDEX_AUTH_ENV = "ANTHROPIC_AUTH_TOKEN";
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

export async function createClaudexProviderConfig(
  env: Readonly<Record<string, string | undefined>> = process.env,
): Promise<ProviderConfig> {
  const settings = resolveSettings(env);
  const models = await loadClaudexModels(settings.configPath);
  return {
    name: "Claudex",
    baseUrl: settings.baseUrl,
    apiKey: settings.apiKey,
    api: "anthropic-messages",
    headers: { [CLAUDEX_ORIGIN_HEADER]: CLAUDEX_ORIGIN_VALUE },
    models,
    refreshModels: async (context?: RefreshModelsContext) =>
      loadClaudexModels(settings.configPath, context?.signal),
  };
}
