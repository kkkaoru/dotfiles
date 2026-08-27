import {
  getSupportedThinkingLevels,
  type Api,
  type Model,
  type ModelThinkingLevel,
  type ThinkingLevel,
} from "@earendil-works/pi-ai";

export const DEFAULT_RAMP_START_RATIO = 0.6;
export const DEFAULT_RESERVE_TOKENS = 16_384;
export const DEFAULT_START_EFFORT: ThinkingLevel = "medium";
export const DEFAULT_RESET_COMPACTION_EFFORT: ThinkingLevel = "xhigh";
export const DEFAULT_RESET_COMPACTION_INTERVAL = 1;

const CANONICAL_LEVELS: readonly ThinkingLevel[] = [
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
];

export interface EffortBoundaries {
  readonly compactionEffort?: ThinkingLevel | undefined;
  readonly endEffort?: ThinkingLevel | undefined;
  readonly startEffort: ThinkingLevel;
}

export interface EffortProfile {
  readonly baseline: ThinkingLevel;
  readonly compaction: ThinkingLevel;
  readonly operational: readonly ThinkingLevel[];
  readonly supported: readonly ThinkingLevel[];
}

export interface EffortSelectionInput {
  readonly contextTokens: number;
  readonly contextWindow: number;
  readonly forceBaseline?: boolean;
  readonly profile: EffortProfile;
  readonly rampStartRatio?: number;
  readonly reserveTokens?: number;
}

function effectiveLevelKey(model: Model<Api>, level: ThinkingLevel): string {
  return model.thinkingLevelMap?.[level] ?? level;
}

function configuredLevel(
  supported: readonly ThinkingLevel[],
  configured: ThinkingLevel,
  fallback: ThinkingLevel,
): ThinkingLevel {
  const exact = supported.find((level): boolean => level === configured);
  if (exact !== undefined) {
    return exact;
  }
  const configuredRank = CANONICAL_LEVELS.indexOf(configured);
  return (
    supported.find((level): boolean => CANONICAL_LEVELS.indexOf(level) >= configuredRank) ??
    fallback
  );
}

function buildProfile(
  supported: readonly ThinkingLevel[],
  boundaries: EffortBoundaries,
): EffortProfile | undefined {
  const deepest = supported.at(-1);
  if (deepest === undefined) {
    return undefined;
  }
  const defaultEnd = supported.at(-2) ?? deepest;
  const baseline = configuredLevel(supported, boundaries.startEffort, defaultEnd);
  const end =
    boundaries.endEffort === undefined
      ? defaultEnd
      : configuredLevel(supported, boundaries.endEffort, defaultEnd);
  const startIndex = supported.indexOf(baseline);
  const operationalEnd = Math.max(startIndex, supported.indexOf(end));
  const compaction =
    boundaries.compactionEffort === undefined
      ? deepest
      : configuredLevel(supported, boundaries.compactionEffort, deepest);
  return {
    baseline,
    compaction,
    operational: supported.slice(startIndex, operationalEnd + 1),
    supported,
  };
}

export function effortProfile(
  model: Model<Api> | null | undefined,
  configuredBoundaries?: EffortBoundaries,
): EffortProfile | undefined {
  if (model?.reasoning !== true) {
    return undefined;
  }
  const available = new Set<ModelThinkingLevel>(getSupportedThinkingLevels(model));
  const distinct = new Map<string, ThinkingLevel>();
  for (const level of CANONICAL_LEVELS) {
    if (available.has(level)) {
      distinct.set(effectiveLevelKey(model, level), level);
    }
  }
  const supported = [...distinct.values()];
  return buildProfile(supported, configuredBoundaries ?? { startEffort: DEFAULT_START_EFFORT });
}

export function compactionLimit(
  contextWindow: number,
  reserveTokens = DEFAULT_RESERVE_TOKENS,
): number {
  return Math.max(1, contextWindow - reserveTokens);
}

export function selectDynamicEffort(input: EffortSelectionInput): ThinkingLevel {
  const { profile } = input;
  if (input.forceBaseline === true || profile.operational.length < 2) {
    return profile.baseline;
  }
  const limit = compactionLimit(input.contextWindow, input.reserveTokens);
  const ratio = Math.max(0, input.contextTokens) / limit;
  const rampStart = Math.min(0.95, Math.max(0, input.rampStartRatio ?? DEFAULT_RAMP_START_RATIO));
  if (ratio <= rampStart) {
    return profile.baseline;
  }
  const step = (1 - rampStart) / (profile.operational.length - 1);
  const index = Math.min(
    profile.operational.length - 1,
    1 + Math.floor((ratio - rampStart) / Math.max(Number.EPSILON, step)),
  );
  return profile.operational[index] ?? profile.baseline;
}

export function effortAtLeast(
  actual: ModelThinkingLevel | undefined,
  threshold: ThinkingLevel,
): boolean {
  return (
    actual !== undefined &&
    actual !== "off" &&
    CANONICAL_LEVELS.indexOf(actual) >= CANONICAL_LEVELS.indexOf(threshold)
  );
}

export function shouldResetAfterCompaction(
  compactionCount: number,
  interval = DEFAULT_RESET_COMPACTION_INTERVAL,
): boolean {
  return compactionCount > 0 && interval > 0 && compactionCount % interval === 0;
}
