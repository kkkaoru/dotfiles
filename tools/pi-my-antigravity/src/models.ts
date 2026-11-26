// This file runs with bun.
import type { ProviderModelConfig } from "@earendil-works/pi-coding-agent";

import { DEFAULT_MODEL } from "./constants.ts";
import { isRecord } from "./json.ts";

/** The subscription bills nothing per token, so every price is zero. */
const FREE_COST = { cacheRead: 0, cacheWrite: 0, input: 0, output: 0 };

// shortcut: limits are the CLI's documented window, not read from the account;
// `refreshModels` replaces the catalog with whatever `gemini-api models list` reports.
const LIMITS = { contextWindow: 1_048_576, maxTokens: 65_536 };

export function modelConfig(id: string): ProviderModelConfig {
  return {
    id,
    name: id,
    input: ["text", "image"],
    reasoning: true,
    cost: FREE_COST,
    ...LIMITS,
  };
}

/** Catalog used until the CLI reports one. */
export function staticModels(): ProviderModelConfig[] {
  return [modelConfig(DEFAULT_MODEL)];
}

/** Ids reported by the CLI, with the static entry first and no duplicates. */
export function mergeDiscovered(ids: readonly string[]): ProviderModelConfig[] {
  const models = staticModels();
  const known = new Set(models.map((model) => model.id));
  for (const id of ids) {
    if (id.length === 0 || known.has(id)) {
      continue;
    }
    known.add(id);
    models.push(modelConfig(id));
  }
  return models;
}

function idOf(value: unknown): string | undefined {
  if (typeof value === "string") {
    return value.replace(/^models\//u, "");
  }
  if (!isRecord(value)) {
    return undefined;
  }
  for (const key of ["id", "name", "model"]) {
    const found = value[key];
    if (typeof found === "string" && found.length > 0) {
      return found.replace(/^models\//u, "");
    }
  }
  return undefined;
}

/** Model ids from `gemini-api models list --output-format json`, in any of its list shapes. */
export function parseModelList(value: unknown): string[] {
  const wrapped = isRecord(value) ? ["models", "data", "items"].map((key) => value[key]) : [];
  const list = Array.isArray(value) ? value : wrapped.find((entry) => Array.isArray(entry));
  if (!Array.isArray(list)) {
    return [];
  }
  const ids = list.map(idOf).filter((id): id is string => id !== undefined);
  return [...new Set(ids)];
}
