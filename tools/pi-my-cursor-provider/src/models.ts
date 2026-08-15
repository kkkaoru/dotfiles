import { Cursor, type SDKModel } from "@cursor/sdk";
import type { ProviderModelConfig } from "@earendil-works/pi-coding-agent";
import type { Credential, RefreshModelsContext } from "@earendil-works/pi-ai";

const DEFAULT_CONTEXT_WINDOW = 256_000;
const DEFAULT_MAX_TOKENS = 16_384;
const CONTEXT_SUFFIX = /^([1-9][0-9]*)(k|m)$/i;

interface FallbackModel {
  id: string;
  name: string;
}

const FALLBACK_MODELS: readonly FallbackModel[] = [
  { id: "auto", name: "Cursor Auto" },
  { id: "composer-2.5", name: "Composer 2.5" },
  { id: "claude-sonnet-4-6", name: "Sonnet 4.6" },
  { id: "claude-opus-5", name: "Opus 5" },
  { id: "gpt-5.6-sol", name: "GPT-5.6 Sol" },
  { id: "gpt-5.4", name: "GPT-5.4" },
  { id: "gemini-3.1-pro", name: "Gemini 3.1 Pro" },
  { id: "grok-4.6", name: "Cursor Grok 4.6" },
  { id: "kimi-k3", name: "Kimi K3" },
  { id: "glm-5.2", name: "GLM 5.2" },
];

function modelConfig(
  id: string,
  name: string,
  contextWindow = DEFAULT_CONTEXT_WINDOW,
): ProviderModelConfig {
  return {
    id,
    name,
    reasoning: false,
    input: ["text", "image"],
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    contextWindow,
    maxTokens: DEFAULT_MAX_TOKENS,
  };
}

export const FALLBACK_CURSOR_MODELS: ProviderModelConfig[] = FALLBACK_MODELS.map(({ id, name }) =>
  modelConfig(id, name),
);

function parseContextWindow(value: string): number | undefined {
  const match = CONTEXT_SUFFIX.exec(value);
  if (!match) return undefined;
  const amount = Number(match[1]);
  const multiplier = match[2]?.toLowerCase() === "m" ? 1_000_000 : 1_000;
  return amount * multiplier;
}

function contextWindowFor(model: SDKModel): number {
  const windows = (model.variants ?? []).flatMap((variant) =>
    variant.params
      .filter((parameter) => parameter.id === "context")
      .map((parameter) => parseContextWindow(parameter.value))
      .filter((value): value is number => value !== undefined),
  );
  return windows.length > 0 ? Math.max(...windows) : DEFAULT_CONTEXT_WINDOW;
}

export function cursorCatalogToModels(catalog: readonly SDKModel[]): ProviderModelConfig[] {
  const models = catalog.map((model) =>
    modelConfig(
      model.id === "default" ? "auto" : model.id,
      model.displayName,
      contextWindowFor(model),
    ),
  );
  return models.some((model) => model.id === "auto")
    ? models
    : [modelConfig("auto", "Cursor Auto"), ...models];
}

function credentialApiKey(credential: Credential | undefined): string | undefined {
  if (credential?.type === "api_key") return credential.key;
  return credential?.access;
}

export async function refreshCursorModels(
  context: RefreshModelsContext,
): Promise<ProviderModelConfig[]> {
  const apiKey = credentialApiKey(context.credential);
  if (!context.allowNetwork || !apiKey) return FALLBACK_CURSOR_MODELS;
  context.signal.throwIfAborted();
  const catalog = await Cursor.models.list({ apiKey });
  context.signal.throwIfAborted();
  return catalog.length > 0 ? cursorCatalogToModels(catalog) : FALLBACK_CURSOR_MODELS;
}
