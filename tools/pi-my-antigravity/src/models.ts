// This file runs with bun.
import type { ProviderModelConfig } from "@earendil-works/pi-coding-agent";

import { PROVIDER_ID } from "./constants.ts";
import {
  DEFAULT_CONTEXT_WINDOW,
  DEFAULT_MAX_TOKENS,
  FAMILIES,
  levelMap,
  type Family,
  type LevelMap,
  type LevelMapSet,
  type ModelThinkingLevel,
  type ThinkingLevel,
} from "./families.ts";

export {
  PROVIDER_ID,
  levelMap,
  type LevelMap,
  type LevelMapSet,
  type ModelThinkingLevel,
  type ThinkingLevel,
};

export interface DiscoveredEntry {
  id: string;
  displayName?: string;
}

const FREE_COST = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 };

const THINKING_BUDGETS: ReadonlyMap<ModelThinkingLevel, number> = new Map([
  ["minimal", 1024],
  ["low", 4096],
  ["medium", 8192],
  ["high", 16384],
  ["xhigh", 24576],
  ["max", 24576],
]);
const DEFAULT_THINKING_BUDGET = 4096;

const RUNTIME_TIER_SUFFIX_PATTERN = /-(low|medium|high|xhigh)$/;

const FAMILY_BY_ID: ReadonlyMap<string, Family> = new Map(
  FAMILIES.map((family) => [family.family, family]),
);
const KNOWN_FAMILY_IDS: ReadonlySet<string> = new Set(FAMILIES.map((family) => family.family));

const FALLBACK_LEVELS: readonly ModelThinkingLevel[] = [
  "high",
  "medium",
  "low",
  "xhigh",
  "minimal",
];

const configFor = (family: Family): ProviderModelConfig => ({
  id: family.family,
  name: family.name,
  reasoning: true,
  thinkingLevelMap: family.levels,
  input: family.input,
  cost: FREE_COST,
  contextWindow: family.contextWindow,
  maxTokens: family.maxTokens,
});

const STATIC_MODELS: readonly ProviderModelConfig[] = FAMILIES.map(configFor);

export const staticModels = (): ProviderModelConfig[] => STATIC_MODELS.slice();

/**
 * Resolve the runtime id the CLI would send for a public id + pi thinking level.
 * `agy --model gemini-4-hydrogen --effort medium` sends `gemini-4-hydrogen-medium`.
 */
export function runtimeModelId(publicId: string, level: ModelThinkingLevel | undefined): string {
  const family = FAMILY_BY_ID.get(publicId);
  if (!family) {
    return publicId;
  }
  if (family.fixedRuntime) {
    return family.fixedRuntime;
  }
  if (level) {
    const requested = family.levels[level];
    if (typeof requested === "string" && requested) {
      return `${family.family}-${requested}`;
    }
  }
  for (const candidate of FALLBACK_LEVELS) {
    const tier = family.levels[candidate];
    if (typeof tier === "string" && tier) {
      return `${family.family}-${tier}`;
    }
  }
  return `${family.family}-low`;
}

/** Thinking budget per level, matching the CLI's `--effort` behaviour. */
export const thinkingBudget = (level: ModelThinkingLevel | undefined): number =>
  (level ? THINKING_BUDGETS.get(level) : undefined) ?? DEFAULT_THINKING_BUDGET;

/**
 * Merge a live `fetchAvailableModels` catalog into the static list: the static families stay
 * (pi keeps a stable model set), and runtime families the account additionally exposes are
 * appended so newly enabled models appear without a package release.
 */
export function mergeDiscovered(discovered: DiscoveredEntry[]): ProviderModelConfig[] {
  const models = staticModels();
  if (discovered.length === 0) {
    return models;
  }

  const extras = new Map<string, string | undefined>();
  for (const model of discovered) {
    const suffix = RUNTIME_TIER_SUFFIX_PATTERN.exec(model.id);
    const family = suffix ? model.id.slice(0, -suffix[0].length) : model.id;
    if (KNOWN_FAMILY_IDS.has(family) || extras.has(family)) {
      continue;
    }
    extras.set(family, model.displayName);
  }

  for (const [family, displayName] of extras) {
    models.push({
      id: family,
      name: displayName ?? `${family} (Antigravity)`,
      reasoning: true,
      thinkingLevelMap: levelMap.threeTier,
      input: ["text", "image"],
      cost: FREE_COST,
      contextWindow: DEFAULT_CONTEXT_WINDOW,
      maxTokens: DEFAULT_MAX_TOKENS,
    });
  }
  return models;
}
