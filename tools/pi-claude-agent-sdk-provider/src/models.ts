// This file runs with Bun.
import type { ProviderModelConfig } from "@earendil-works/pi-coding-agent";

interface ModelSpec {
  readonly id: string;
  readonly name: string;
  readonly contextWindow: number;
  readonly maxTokens: number;
  readonly reasoning: boolean;
}

// Plan usage carries no per-token price for pi to report.
const ZERO_COST = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 };

// Canonical ids from the SDK's `supportedModels()` for a Claude plan login (aliases omitted).
const MODEL_SPECS: readonly ModelSpec[] = [
  {
    id: "claude-opus-5-5",
    name: "Claude Opus 5.5",
    contextWindow: 1_000_000,
    maxTokens: 128_000,
    reasoning: true,
  },
  {
    id: "claude-fable-5-1",
    name: "Claude Fable 5.1",
    contextWindow: 1_000_000,
    maxTokens: 128_000,
    reasoning: true,
  },
  {
    id: "claude-sonnet-5-5",
    name: "Claude Sonnet 5.5",
    contextWindow: 1_000_000,
    maxTokens: 128_000,
    reasoning: true,
  },
  {
    id: "claude-haiku-5-5",
    name: "Claude Haiku 5.5",
    contextWindow: 1_000_000,
    maxTokens: 128_000,
    reasoning: true,
  },
  {
    id: "claude-opus-5",
    name: "Claude Opus 5",
    contextWindow: 1_000_000,
    maxTokens: 128_000,
    reasoning: true,
  },
  {
    id: "claude-fable-5",
    name: "Claude Fable 5",
    contextWindow: 1_000_000,
    maxTokens: 128_000,
    reasoning: true,
  },
  {
    id: "claude-sonnet-5",
    name: "Claude Sonnet 5",
    contextWindow: 1_000_000,
    maxTokens: 128_000,
    reasoning: true,
  },
  {
    id: "claude-opus-4-8",
    name: "Claude Opus 4.8",
    contextWindow: 1_000_000,
    maxTokens: 128_000,
    reasoning: true,
  },
  {
    id: "claude-opus-4-7",
    name: "Claude Opus 4.7",
    contextWindow: 1_000_000,
    maxTokens: 128_000,
    reasoning: true,
  },
  {
    id: "claude-opus-4-6",
    name: "Claude Opus 4.6",
    contextWindow: 1_000_000,
    maxTokens: 128_000,
    reasoning: true,
  },
  {
    id: "claude-sonnet-4-6",
    name: "Claude Sonnet 4.6",
    contextWindow: 1_000_000,
    maxTokens: 128_000,
    reasoning: true,
  },
  // No adaptive thinking or effort support, so pi must not request reasoning.
  {
    id: "claude-haiku-4-5-20251001",
    name: "Claude Haiku 4.5",
    contextWindow: 200_000,
    maxTokens: 64_000,
    reasoning: false,
  },
];

function toProviderModel(spec: ModelSpec): ProviderModelConfig {
  return {
    id: spec.id,
    name: `${spec.name} (Agent SDK)`,
    reasoning: spec.reasoning,
    input: ["text", "image"],
    cost: ZERO_COST,
    contextWindow: spec.contextWindow,
    maxTokens: spec.maxTokens,
  };
}

export const CLAUDE_SDK_MODELS: ProviderModelConfig[] = MODEL_SPECS.map((spec) =>
  toProviderModel(spec),
);
