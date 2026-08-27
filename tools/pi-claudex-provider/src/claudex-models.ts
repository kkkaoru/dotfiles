import { readFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import type { ProviderModelConfig } from "@earendil-works/pi-coding-agent";
import { isRecord, type JsonRecord } from "./protocol.ts";

const DEFAULT_CONTEXT_WINDOW = 200_000;
const DEFAULT_MAX_TOKENS = 32_768;
const ZERO_COST = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 };

interface ModelCandidate {
  id: string;
  contextWindow: number;
}

function expandHome(filePath: string): string {
  if (filePath === "~") {
    return os.homedir();
  }
  if (filePath.startsWith("~/")) {
    return path.join(os.homedir(), filePath.slice(2));
  }
  return filePath;
}

function optionalString(record: JsonRecord, key: string): string | undefined {
  const value = record[key];
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function contextWindow(record: JsonRecord): number {
  const value = record["maxContextTokens"];
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0
    ? value
    : DEFAULT_CONTEXT_WINDOW;
}

function appendCandidate(
  candidates: ModelCandidate[],
  seen: Set<string>,
  id: string | undefined,
  window: number,
): void {
  if (id === undefined || seen.has(id)) {
    return;
  }
  seen.add(id);
  candidates.push({ id, contextWindow: window });
}

function appendProviderModels(
  candidates: ModelCandidate[],
  seen: Set<string>,
  value: unknown,
): void {
  if (!isRecord(value) || value["enabled"] === false) {
    return;
  }
  const window = contextWindow(value);
  appendCandidate(candidates, seen, optionalString(value, "defaultModel"), window);
  appendCandidate(candidates, seen, optionalString(value, "subagentModel"), window);
  const selectable = value["selectableModels"];
  if (Array.isArray(selectable)) {
    for (const model of selectable) {
      appendCandidate(candidates, seen, typeof model === "string" ? model : undefined, window);
    }
  }
}

function appendNamedModel(candidates: ModelCandidate[], seen: Set<string>, value: unknown): void {
  if (!isRecord(value)) {
    return;
  }
  appendCandidate(candidates, seen, optionalString(value, "model"), contextWindow(value));
}

function collectCandidates(config: JsonRecord): ModelCandidate[] {
  const candidates: ModelCandidate[] = [];
  const seen = new Set<string>();
  const { providers } = config;
  if (Array.isArray(providers)) {
    for (const provider of providers) {
      appendProviderModels(candidates, seen, provider);
    }
  }
  const { nativeWorkers } = config;
  if (Array.isArray(nativeWorkers)) {
    for (const worker of nativeWorkers) {
      appendNamedModel(candidates, seen, worker);
    }
  }
  appendNamedModel(candidates, seen, config["fallback"]);
  appendNamedModel(candidates, seen, config["advisor"]);
  return candidates;
}

function toProviderModel(candidate: ModelCandidate): ProviderModelConfig {
  return {
    id: candidate.id,
    name: `Claudex · ${candidate.id}`,
    reasoning: true,
    compat: { forceAdaptiveThinking: true },
    thinkingLevelMap: {
      off: null,
      minimal: "minimal",
      low: "low",
      medium: "medium",
      high: "high",
      xhigh: "xhigh",
      max: "max",
    },
    input: ["text", "image"],
    cost: ZERO_COST,
    contextWindow: candidate.contextWindow,
    maxTokens: DEFAULT_MAX_TOKENS,
  };
}

function throwIfAborted(signal: AbortSignal | undefined): void {
  if (signal?.aborted === true) {
    throw new DOMException("Claudex model refresh was aborted", "AbortError");
  }
}

export async function loadClaudexModels(
  configPath: string,
  signal?: AbortSignal,
): Promise<ProviderModelConfig[]> {
  throwIfAborted(signal);
  const text = await readFile(expandHome(configPath), "utf8");
  throwIfAborted(signal);
  const parsed: unknown = JSON.parse(text);
  if (!isRecord(parsed)) {
    throw new Error("Claudex provider config must be a JSON object");
  }
  const models = collectCandidates(parsed).map((candidate) => toProviderModel(candidate));
  if (models.length === 0) {
    throw new Error("Claudex provider config contains no enabled models");
  }
  return models;
}
