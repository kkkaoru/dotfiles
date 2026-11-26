// This file runs with bun.
import type { ModelThinkingLevel, ThinkingLevel, ThinkingLevelMap } from "@earendil-works/pi-ai";

export type { ModelThinkingLevel, ThinkingLevel, ThinkingLevelMap };

export type LevelMap = ThinkingLevelMap;

export interface LevelMapSet {
  threeTier: LevelMap;
  fourTier: LevelMap;
  mediumOnly: LevelMap;
  highOnly: LevelMap;
}

export interface Family {
  /** Public pi model id; runtime ids are `<family>-<level>` unless `fixedRuntime` is set. */
  family: string;
  name: string;
  levels: LevelMap;
  contextWindow: number;
  maxTokens: number;
  input: Array<"text" | "image">;
  fixedRuntime?: string;
}

export const DEFAULT_CONTEXT_WINDOW = 1048576;
export const DEFAULT_MAX_TOKENS = 65536;
const CLAUDE_MAX_TOKENS = 128000;
const GPT_OSS_CONTEXT_WINDOW = 131072;
const GPT_OSS_MAX_TOKENS = 32768;
const HYDROGEN_MAX_TOKENS = 262144;

const withLevels = (...levels: Array<Exclude<ThinkingLevel, "minimal" | "max">>): LevelMap => {
  const map: Record<ModelThinkingLevel, string | null> = {
    off: null,
    minimal: null,
    low: null,
    medium: null,
    high: null,
    xhigh: null,
    max: null,
  };
  for (const level of levels) {
    map[level] = level;
  }
  return map;
};

export const levelMap: LevelMapSet = {
  threeTier: withLevels("low", "medium", "high"),
  fourTier: withLevels("low", "medium", "high", "xhigh"),
  mediumOnly: withLevels("medium"),
  highOnly: withLevels("high"),
};

/** Mirrors `agy models` on CLI 1.5.9; discovery appends any additional account families. */
export const FAMILIES: readonly Family[] = [
  {
    family: "gemini-4-hydrogen",
    name: "Gemini 4 Hydrogen (Antigravity)",
    levels: levelMap.fourTier,
    contextWindow: DEFAULT_CONTEXT_WINDOW,
    maxTokens: HYDROGEN_MAX_TOKENS,
    input: ["text", "image"],
  },
  {
    family: "gemini-4-argon",
    name: "Gemini 4 Argon (Antigravity)",
    levels: levelMap.threeTier,
    contextWindow: DEFAULT_CONTEXT_WINDOW,
    maxTokens: DEFAULT_MAX_TOKENS,
    input: ["text", "image"],
  },
  {
    family: "gemini-3.8-flash",
    name: "Gemini 3.8 Flash (Antigravity)",
    levels: levelMap.threeTier,
    contextWindow: DEFAULT_CONTEXT_WINDOW,
    maxTokens: DEFAULT_MAX_TOKENS,
    input: ["text", "image"],
  },
  {
    family: "claude-opus-5-5",
    name: "Claude Opus 5.5 (Antigravity)",
    levels: levelMap.threeTier,
    contextWindow: DEFAULT_CONTEXT_WINDOW,
    maxTokens: CLAUDE_MAX_TOKENS,
    input: ["text", "image"],
  },
  {
    family: "claude-sonnet-5-5",
    name: "Claude Sonnet 5.5 (Antigravity)",
    levels: levelMap.threeTier,
    contextWindow: DEFAULT_CONTEXT_WINDOW,
    maxTokens: CLAUDE_MAX_TOKENS,
    input: ["text", "image"],
  },
  {
    family: "gpt-oss-120b",
    name: "GPT-OSS 120B (Antigravity)",
    levels: levelMap.mediumOnly,
    contextWindow: GPT_OSS_CONTEXT_WINDOW,
    maxTokens: GPT_OSS_MAX_TOKENS,
    input: ["text"],
  },
  {
    family: "gemini-pro-orchestrator",
    name: "Orchestrator (Argon, Antigravity)",
    levels: levelMap.highOnly,
    contextWindow: DEFAULT_CONTEXT_WINDOW,
    maxTokens: DEFAULT_MAX_TOKENS,
    input: ["text", "image"],
    fixedRuntime: "gemini-pro-orchestrator-g1-pro",
  },
];
