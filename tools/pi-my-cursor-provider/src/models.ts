import { Cursor, type ModelParameterValue, type ModelSelection, type SDKModel } from "@cursor/sdk";
import type { ProviderModelConfig } from "@earendil-works/pi-coding-agent";
import type { Credential, RefreshModelsContext, ThinkingLevel } from "@earendil-works/pi-ai";

const DEFAULT_CONTEXT_WINDOW = 256_000;
const DEFAULT_MAX_TOKENS = 16_384;
const CONTEXT_SUFFIX = /^([1-9][0-9]*)(k|m)$/i;
/**
 * Cursor reports oversized requests as usage-guideline blocks instead of
 * recognizable overflow errors, so overflow recovery cannot rescue a session
 * that already passed the real window. Advertise a reduced window so pi's
 * native auto-compaction fires comfortably below the actual 256k limit.
 */
const CONTEXT_WINDOW_SAFETY = 0.8;
const EFFORT_PARAMETER_IDS = new Set(["effort", "reasoning"]);
const EFFORT_PREFERENCES: Record<ThinkingLevel, readonly string[]> = {
  minimal: ["minimal", "low", "none"],
  low: ["low", "minimal", "none"],
  medium: ["medium", "low", "high"],
  high: ["high", "xhigh", "max", "medium"],
  xhigh: ["xhigh", "max", "high"],
  max: ["max", "xhigh", "high"],
};

interface EffortCapability {
  readonly defaults: readonly ModelParameterValue[];
  readonly parameterId: string;
  readonly values: ReadonlySet<string>;
}

interface FallbackModel {
  id: string;
  name: string;
}

const effortCapabilities = new Map<string, EffortCapability>();
const warnedUnsupportedEffort = new Set<string>();
let catalogLoaded = false;
let catalogLoad: Promise<void> | undefined;

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
  reasoning = false,
): ProviderModelConfig {
  return {
    id,
    name,
    reasoning,
    input: ["text", "image"],
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    contextWindow: effectiveContextWindow(contextWindow),
    maxTokens: DEFAULT_MAX_TOKENS,
  };
}

/** Effective window pi plans against: a safety margin below the real one. */
export function effectiveContextWindow(contextWindow: number): number {
  return Math.floor(contextWindow * CONTEXT_WINDOW_SAFETY);
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

export function recordCursorCatalog(catalog: readonly SDKModel[]): void {
  effortCapabilities.clear();
  catalogLoaded = true;
  for (const model of catalog) {
    const parameter = model.parameters?.find(({ id }) => EFFORT_PARAMETER_IDS.has(id));
    if (!parameter) continue;
    effortCapabilities.set(model.id === "default" ? "auto" : model.id, {
      defaults: model.variants?.find(({ isDefault }) => isDefault)?.params ?? [],
      parameterId: parameter.id,
      values: new Set(parameter.values.map(({ value }) => value)),
    });
  }
}

function warnUnsupportedEffort(modelId: string, effort: ThinkingLevel): void {
  const key = `${modelId}:${effort}`;
  if (warnedUnsupportedEffort.has(key)) return;
  warnedUnsupportedEffort.add(key);
  console.warn(
    `Cursor model ${modelId} does not expose a compatible effort parameter; requested ${effort} was not forwarded.`,
  );
}

async function ensureCursorCatalog(apiKey: string | undefined): Promise<void> {
  if (catalogLoaded) return;
  catalogLoad ??= Cursor.models
    .list(apiKey ? { apiKey } : undefined)
    .then(recordCursorCatalog)
    .finally(() => {
      catalogLoad = undefined;
    });
  await catalogLoad;
}

export async function cursorModelSelection(
  modelId: string,
  effort: ThinkingLevel | undefined,
  apiKey?: string,
): Promise<ModelSelection> {
  if (!effort) return { id: modelId };
  await ensureCursorCatalog(apiKey);
  const capability = effortCapabilities.get(modelId);
  const value = EFFORT_PREFERENCES[effort].find((candidate) => capability?.values.has(candidate));
  if (!capability || !value) {
    warnUnsupportedEffort(modelId, effort);
    return { id: modelId };
  }

  let replaced = false;
  const params = capability.defaults.map((parameter) => {
    if (parameter.id !== capability.parameterId) return parameter;
    replaced = true;
    return { id: capability.parameterId, value };
  });
  if (!replaced) params.push({ id: capability.parameterId, value });
  return { id: modelId, params };
}

export function cursorCatalogToModels(catalog: readonly SDKModel[]): ProviderModelConfig[] {
  const models = catalog.map((model) =>
    modelConfig(
      model.id === "default" ? "auto" : model.id,
      model.displayName,
      contextWindowFor(model),
      model.parameters?.some(({ id }) => EFFORT_PARAMETER_IDS.has(id)) ?? false,
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
  if (catalog.length === 0) return FALLBACK_CURSOR_MODELS;
  recordCursorCatalog(catalog);
  return cursorCatalogToModels(catalog);
}
