// This file runs with Bun.
import { execFile } from "node:child_process";
import type { ProviderModelConfig } from "@earendil-works/pi-coding-agent";
import type { RefreshModelsContext } from "@earendil-works/pi-ai";

interface FallbackModel {
  contextWindow: number;
  id: string;
  maxTokens: number;
  name: string;
}

interface ModelCost {
  cacheRead: number;
  cacheWrite: number;
  input: number;
  output: number;
}

interface CommandResult {
  stderr: string;
  stdout: string;
}

const DEVIN_COMMAND: string = "devin";
const MODEL_ARGUMENTS: string[] = ["models", "list", "--format", "json"];
const CONTEXT_WINDOW_SAFETY: number = 0.8;
const DEFAULT_CONTEXT_WINDOW: number = 262_000;
const DEFAULT_MAX_TOKENS: number = 128_000;
const COST_PATTERN: RegExp = /\$([0-9.]+)\s*\/\s*MTok In.*\$([0-9.]+)\s*\/\s*MTok Out/i;
const ZERO_COST: ModelCost = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 };
const FALLBACK_CATALOG: readonly FallbackModel[] = [
  {
    id: "adaptive",
    name: "Adaptive",
    contextWindow: DEFAULT_CONTEXT_WINDOW,
    maxTokens: DEFAULT_MAX_TOKENS,
  },
  {
    id: "swe-1-7",
    name: "SWE-1.7 Max",
    contextWindow: 262_000,
    maxTokens: 128_000,
  },
  {
    id: "swe-1-7-medium",
    name: "SWE-1.7 Medium",
    contextWindow: 262_000,
    maxTokens: 128_000,
  },
  {
    id: "claude-sonnet-5-medium",
    name: "Claude Sonnet 5 Medium",
    contextWindow: 1_000_000,
    maxTokens: 128_000,
  },
  {
    id: "claude-opus-5-medium",
    name: "Claude Opus 5 Medium",
    contextWindow: 1_000_000,
    maxTokens: 128_000,
  },
  {
    id: "gpt-5-6-luna-medium",
    name: "GPT-5.6 Luna Medium Thinking",
    contextWindow: 1_000_000,
    maxTokens: 128_000,
  },
  {
    id: "gemini-3-7-flash-medium",
    name: "Gemini 3.7 Flash Medium",
    contextWindow: 1_048_576,
    maxTokens: 65_535,
  },
  {
    id: "glm-5-2",
    name: "GLM-5.2 High",
    contextWindow: 200_000,
    maxTokens: 128_000,
  },
  {
    id: "kimi-k3-high",
    name: "Kimi K3 High",
    contextWindow: 1_048_576,
    maxTokens: 131_072,
  },
];

function effectiveContextWindow(contextWindow: number): number {
  return Math.floor(contextWindow * CONTEXT_WINDOW_SAFETY);
}

function modelConfig(model: FallbackModel, cost: ModelCost): ProviderModelConfig {
  return {
    id: model.id,
    name: model.name,
    reasoning: true,
    input: ["text", "image"],
    cost,
    contextWindow: effectiveContextWindow(model.contextWindow),
    maxTokens: model.maxTokens,
  };
}

export const FALLBACK_DEVIN_MODELS: ProviderModelConfig[] = FALLBACK_CATALOG.map((model) =>
  modelConfig(model, ZERO_COST),
);

function restoreModels(context: RefreshModelsContext): ProviderModelConfig[] {
  const stored = context.stored?.models;
  return stored === undefined || stored.length === 0
    ? FALLBACK_DEVIN_MODELS
    : stored.map((model) => ({ ...model, input: [...model.input] }));
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function stringValue(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function positiveNumber(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) && value > 0 ? value : fallback;
}

function parseCost(value: unknown): ModelCost {
  const summary: string | undefined = stringValue(value);
  const match: RegExpExecArray | null = summary ? COST_PATTERN.exec(summary) : null;
  const input: number = Number(match?.[1]);
  const output: number = Number(match?.[2]);
  return Number.isFinite(input) && Number.isFinite(output)
    ? { input, output, cacheRead: 0, cacheWrite: 0 }
    : ZERO_COST;
}

function variantToModel(value: unknown): ProviderModelConfig[] {
  if (!isRecord(value)) return [];
  const id: string | undefined = stringValue(value["model_uid"]);
  const name: string | undefined = stringValue(value["label"]);
  if (!id || !name) return [];
  return [
    modelConfig(
      {
        id,
        name,
        contextWindow: positiveNumber(value["max_context_tokens"], DEFAULT_CONTEXT_WINDOW),
        maxTokens: positiveNumber(value["max_output_tokens"], DEFAULT_MAX_TOKENS),
      },
      parseCost(value["cost_summary"]),
    ),
  ];
}

export function parseDevinModelCatalog(value: unknown): ProviderModelConfig[] {
  if (!isRecord(value) || !Array.isArray(value["families"])) return [];
  return value["families"].flatMap((family): ProviderModelConfig[] => {
    if (!isRecord(family) || !Array.isArray(family["variants"])) return [];
    return family["variants"].flatMap(variantToModel);
  });
}

function runModelCommand(signal: AbortSignal): Promise<CommandResult> {
  return new Promise((resolve, reject) => {
    execFile(
      DEVIN_COMMAND,
      MODEL_ARGUMENTS,
      { encoding: "utf8", signal },
      (...callbackArguments) => {
        const [error, stdout, stderr] = callbackArguments;
        if (error) {
          reject(error);
          return;
        }
        resolve({ stdout, stderr });
      },
    );
  });
}

export async function refreshDevinModels(
  context: RefreshModelsContext,
): Promise<ProviderModelConfig[]> {
  context.signal.throwIfAborted();
  if (!context.allowNetwork) return restoreModels(context);
  const result: CommandResult = await runModelCommand(context.signal);
  context.signal.throwIfAborted();
  const models: ProviderModelConfig[] = parseDevinModelCatalog(JSON.parse(result.stdout));
  return models.length > 0 ? models : FALLBACK_DEVIN_MODELS;
}
