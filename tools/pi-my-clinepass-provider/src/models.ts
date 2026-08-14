/**
 * ClinePass model definitions and dynamic model discovery.
 *
 * @module clinepass-models
 */

import type { ThinkingLevelMap } from "@earendil-works/pi-ai";
import type { ClineCatalogModel } from "./cline-models.ts";

// ─── Model Definitions ─────────────────────────────────────────────────────

/**
 * Default thinking level map for remote models without a static fallback.
 * Assumes low/medium/high are supported and marks minimal/xhigh unsupported.
 * "off" maps to "none" for the ClinePass API.
 */
export const DEFAULT_THINKING_LEVEL_MAP: ThinkingLevelMap = {
  off: "none",
  minimal: null,
  low: "low",
  medium: "medium",
  high: "high",
  xhigh: null,
};

/**
 * All-null thinking level map used when a model reports reasoning: false.
 * Every level is unsupported — reasoning is simply not available.
 */
export const NO_THINKING_MAP: ThinkingLevelMap = {
  off: null,
  minimal: null,
  low: null,
  medium: null,
  high: null,
  xhigh: null,
};

/**
 * OpenAI-compat flags for ClinePass chat completions.
 *
 * ClinePass only accepts classic roles (`system`, `assistant`, `user`, `tool`,
 * `function`). pi-ai defaults to `developer` for reasoning models unless
 * `supportsDeveloperRole` is false (see pi-ai README).
 */
export interface ClinePassOpenAICompat {
  readonly supportsDeveloperRole: boolean;
  readonly thinkingFormat?: string;
}

export const CLINEPASS_OPENAI_COMPAT: ClinePassOpenAICompat = {
  supportsDeveloperRole: false,
};

/**
 * ClinePass curated open-weight coding models.
 *
 * Model IDs use the full ClinePass slug (e.g. "cline-pass/glm-5.2") as
 * documented at https://docs.cline.bot/getting-started/clinepass — these are
 * the values Cline's API expects in the `model` field.
 *
 * `contextWindow` is in tokens; `maxTokens` is the max output tokens.
 * Reference pricing ($/M tokens) is from the ClinePass docs and is used for
 * usage tracking — ClinePass itself is a flat $9.99/mo subscription.
 */
export interface ModelConfig {
  id: string;
  name: string;
  reasoning: boolean;
  input: readonly ("text" | "image")[];
  cost: { input: number; output: number; cacheRead: number; cacheWrite: number };
  contextWindow: number;
  maxTokens: number;
  /** Maps supported pi thinking levels to ClinePass reasoning_effort values. */
  thinkingLevelMap: ThinkingLevelMap;
  /** Pi-ai openai-completions compat overrides for the ClinePass API. */
  compat: ClinePassOpenAICompat;
}

/** Static catalog entries; per-model compat overrides merge with CLINEPASS_OPENAI_COMPAT. */
interface ModelConfigBase extends Omit<ModelConfig, "compat"> {
  compat?: Partial<ClinePassOpenAICompat>;
}

const MODELS_BASE: readonly ModelConfigBase[] = [
  {
    id: "cline-pass/glm-5.3",
    name: "GLM-5.3 (ClinePass)",
    reasoning: true,
    input: ["text"],
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    contextWindow: 128_000,
    maxTokens: 8192,
    thinkingLevelMap: NO_THINKING_MAP,
  },
  {
    id: "cline-pass/glm-5.2",
    name: "GLM-5.2 (ClinePass)",
    reasoning: true,
    input: ["text"],
    cost: { input: 1.4, output: 4.4, cacheRead: 0.26, cacheWrite: 0 },
    contextWindow: 200_000,
    maxTokens: 131_072,
    thinkingLevelMap: {
      off: "none",
      minimal: null,
      low: "low",
      medium: "medium",
      high: "high",
      xhigh: "xhigh",
    },
  },
  {
    id: "cline-pass/kimi-k2.7-code",
    name: "Kimi K2.7 Code (ClinePass)",
    reasoning: true,
    input: ["text"],
    cost: { input: 0.95, output: 4, cacheRead: 0.19, cacheWrite: 0 },
    contextWindow: 262_144,
    maxTokens: 131_072,
    thinkingLevelMap: {
      off: null,
      minimal: null,
      low: "low",
      medium: "medium",
      high: "high",
      xhigh: null,
    },
  },
  {
    id: "cline-pass/kimi-k2.6",
    name: "Kimi K2.6 (ClinePass)",
    reasoning: true,
    input: ["text"],
    cost: { input: 0.95, output: 4, cacheRead: 0.16, cacheWrite: 0 },
    contextWindow: 262_144,
    maxTokens: 131_072,
    thinkingLevelMap: {
      off: null,
      minimal: null,
      low: "low",
      medium: "medium",
      high: "high",
      xhigh: null,
    },
  },
  {
    id: "cline-pass/kimi-k3",
    name: "Kimi K3 (ClinePass)",
    reasoning: true,
    input: ["text"],
    cost: { input: 3, output: 15, cacheRead: 0.3, cacheWrite: 0 },
    contextWindow: 1_048_576,
    maxTokens: 131_072,
    // K3 always reasons and currently only supports reasoning_effort="max".
    thinkingLevelMap: {
      off: null,
      minimal: null,
      low: null,
      medium: null,
      high: "max",
      xhigh: null,
    },
  },
  {
    id: "cline-pass/deepseek-v4-pro",
    name: "DeepSeek V4 Pro (ClinePass)",
    reasoning: true,
    input: ["text"],
    cost: { input: 1.74, output: 3.48, cacheRead: 0.0145, cacheWrite: 0 },
    contextWindow: 1_000_000,
    maxTokens: 384_000,
    thinkingLevelMap: {
      off: "none",
      minimal: null,
      low: null,
      medium: null,
      high: "high",
      xhigh: "high",
    },
  },
  {
    id: "cline-pass/deepseek-v4-flash",
    name: "DeepSeek V4 Flash (ClinePass)",
    reasoning: true,
    input: ["text"],
    cost: { input: 0.14, output: 0.28, cacheRead: 0.0028, cacheWrite: 0 },
    contextWindow: 1_000_000,
    maxTokens: 384_000,
    thinkingLevelMap: {
      off: "none",
      minimal: null,
      low: null,
      medium: null,
      high: "high",
      xhigh: "high",
    },
  },
  {
    id: "cline-pass/mimo-v2.5",
    name: "MiMo-V2.5 (ClinePass)",
    reasoning: true,
    input: ["text"],
    cost: { input: 0.14, output: 0.28, cacheRead: 0.0028, cacheWrite: 0 },
    contextWindow: 262_144,
    maxTokens: 131_072,
    thinkingLevelMap: {
      off: "none",
      minimal: null,
      low: "low",
      medium: "medium",
      high: "high",
      xhigh: null,
    },
  },
  {
    id: "cline-pass/mimo-v2.5-pro",
    name: "MiMo-V2.5-Pro (ClinePass)",
    reasoning: true,
    input: ["text"],
    cost: { input: 1.74, output: 3.48, cacheRead: 0.0145, cacheWrite: 0 },
    contextWindow: 262_144,
    maxTokens: 131_072,
    thinkingLevelMap: {
      off: "none",
      minimal: null,
      low: "low",
      medium: "medium",
      high: "high",
      xhigh: null,
    },
  },
  {
    id: "cline-pass/minimax-m3",
    name: "MiniMax M3 (ClinePass)",
    reasoning: true,
    input: ["text"],
    cost: { input: 0.3, output: 1.2, cacheRead: 0.06, cacheWrite: 0 },
    contextWindow: 1_048_576,
    maxTokens: 131_072,
    thinkingLevelMap: {
      off: "none",
      minimal: null,
      low: "low",
      medium: "medium",
      high: "high",
      xhigh: null,
    },
  },
  {
    id: "cline-pass/qwen3.8-max",
    name: "Qwen3.8 Max (ClinePass)",
    reasoning: true,
    input: ["text", "image"],
    cost: { input: 2, output: 6, cacheRead: 0.25, cacheWrite: 2.5 },
    contextWindow: 1_000_000,
    maxTokens: 131_072,
    thinkingLevelMap: {
      off: null,
      minimal: "minimal",
      low: "low",
      medium: "medium",
      high: "high",
      xhigh: "xhigh",
      max: null,
    },
  },
  {
    id: "cline-pass/qwen3.7-max",
    name: "Qwen3.7 Max (ClinePass)",
    reasoning: true,
    input: ["text"],
    cost: { input: 2.5, output: 7.5, cacheRead: 0.5, cacheWrite: 3.125 },
    contextWindow: 262_144,
    maxTokens: 131_072,
    thinkingLevelMap: {
      off: "none",
      minimal: null,
      low: "low",
      medium: "medium",
      high: "high",
      xhigh: null,
    },
  },
  {
    id: "cline-pass/qwen3.7-plus",
    name: "Qwen3.7 Plus (ClinePass)",
    reasoning: true,
    input: ["text"],
    // Qwen3.7 Plus has tiered pricing; we use the ≤256K rate as the default.
    cost: { input: 0.4, output: 1.6, cacheRead: 0.04, cacheWrite: 0.5 },
    contextWindow: 1_048_576,
    maxTokens: 131_072,
    thinkingLevelMap: {
      off: "none",
      minimal: null,
      low: "low",
      medium: "medium",
      high: "high",
      xhigh: null,
    },
  },
];

function withCompat(model: ModelConfigBase): ModelConfig {
  return {
    ...model,
    compat: {
      ...CLINEPASS_OPENAI_COMPAT,
      ...model.compat,
    },
  };
}

export const MODELS: readonly ModelConfig[] = MODELS_BASE.map((model) => withCompat(model));

/**
 * Return the model IDs registered for the ClinePass provider.
 */
export function modelIds(): string[] {
  return MODELS.map((model) => model.id);
}

function catalogModel(
  entry: ClineCatalogModel,
  metadataById: ReadonlyMap<string, ModelConfig>,
): ModelConfig {
  const metadata = metadataById.get(entry.modelId);
  if (metadata !== undefined) {
    const name = entry.name === entry.modelId ? metadata.name : `${entry.name} (ClinePass)`;
    return { ...metadata, name };
  }
  return {
    id: entry.modelId,
    name:
      entry.name === entry.modelId ? `${entry.modelId} (ClinePass)` : `${entry.name} (ClinePass)`,
    reasoning: true,
    input: ["text"],
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    contextWindow: 128_000,
    maxTokens: 8192,
    thinkingLevelMap: DEFAULT_THINKING_LEVEL_MAP,
    compat: CLINEPASS_OPENAI_COMPAT,
  };
}

export function modelsFromClineCatalog(catalog: readonly ClineCatalogModel[]): ModelConfig[] {
  const metadataById = new Map(MODELS.map((model) => [model.id, model]));
  const unique = new Map(catalog.map((entry) => [entry.modelId, entry]));
  return [...unique.values()].map((entry) => catalogModel(entry, metadataById));
}
