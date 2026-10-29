// This TypeScript file is executed with Bun.
import type { ModelThinkingLevel, ThinkingLevel } from "@earendil-works/pi-ai";
import { CANONICAL_LEVELS, effortAtLeast, shouldResetAfterCompaction } from "./policy.ts";
import { readRecoveryState, type RecoveryState } from "./recovery-effort.ts";
import { isRecord } from "./settings.ts";

export const STATE_ENTRY_TYPE = "pi-effort-manager-state-v1";

const THINKING_LEVELS: ReadonlySet<string> = new Set(CANONICAL_LEVELS);

export interface SessionOverrides {
  compactionEffort?: ThinkingLevel | undefined;
  compactionResetEffort?: ThinkingLevel | undefined;
  compactionResetInterval?: number | undefined;
  endEffort?: ThinkingLevel | undefined;
  startEffort?: ThinkingLevel | undefined;
}

export interface ResetPolicy {
  effort: ThinkingLevel;
  interval: number;
}

/** A finished compaction: the level it ran after, and the count before it. */
export interface CompactionCount extends ResetPolicy {
  readonly level: ModelThinkingLevel;
  readonly count: number;
}

export interface CompactionOutcome {
  readonly count: number;
  readonly reset: boolean;
}

export interface ResetPolicyDefaults {
  compactionResetEffort: ThinkingLevel;
  compactionResetInterval: number;
}

export interface PersistedState {
  recovery?: RecoveryState;
  compactionCount?: number | undefined;
  enabled?: boolean | undefined;
  overrides?: SessionOverrides | undefined;
  resetEffort?: ThinkingLevel | undefined;
  resetInterval?: number | undefined;
}

export interface ManagerState {
  compactionCount: number;
  enabled: boolean;
  forceBaselineTurns: number;
  overrides: SessionOverrides;
  preCompactionLevel: ModelThinkingLevel | undefined;
}

export function initialManagerState(enabled: boolean): ManagerState {
  return {
    compactionCount: 0,
    enabled,
    forceBaselineTurns: 0,
    overrides: {},
    preCompactionLevel: undefined,
  };
}

function thinkingLevel(value: unknown): ThinkingLevel | undefined {
  return typeof value === "string" && THINKING_LEVELS.has(value)
    ? (value as ThinkingLevel)
    : undefined;
}

function positiveInteger(value: unknown): number | undefined {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0 ? value : undefined;
}

function sessionOverrides(value: unknown): SessionOverrides | undefined {
  if (!isRecord(value)) {
    return undefined;
  }
  return {
    compactionEffort: thinkingLevel(value["compactionEffort"]),
    compactionResetEffort: thinkingLevel(value["compactionResetEffort"]),
    compactionResetInterval: positiveInteger(value["compactionResetInterval"]),
    endEffort: thinkingLevel(value["endEffort"]),
    startEffort: thinkingLevel(value["startEffort"]),
  };
}

function persistedState(entry: unknown): PersistedState | undefined {
  if (!isRecord(entry) || entry["type"] !== "custom" || entry["customType"] !== STATE_ENTRY_TYPE) {
    return undefined;
  }
  const { data } = entry;
  if (!isRecord(data)) {
    return undefined;
  }
  return {
    compactionCount:
      typeof data["compactionCount"] === "number" &&
      Number.isSafeInteger(data["compactionCount"]) &&
      data["compactionCount"] >= 0
        ? data["compactionCount"]
        : undefined,
    recovery: readRecoveryState(data["recovery"]),
    enabled: typeof data["enabled"] === "boolean" ? data["enabled"] : undefined,
    overrides: sessionOverrides(data["overrides"]),
    resetEffort: thinkingLevel(data["resetEffort"]),
    resetInterval: positiveInteger(data["resetInterval"]),
  };
}

export function latestPersistedState(entries: readonly unknown[]): PersistedState {
  return (
    persistedState(entries.findLast((entry): boolean => persistedState(entry) !== undefined)) ?? {}
  );
}

export function effectiveResetPolicy(
  { compactionResetEffort, compactionResetInterval }: SessionOverrides,
  defaults: ResetPolicyDefaults,
): ResetPolicy {
  return {
    effort: compactionResetEffort ?? defaults.compactionResetEffort,
    interval: compactionResetInterval ?? defaults.compactionResetInterval,
  };
}

export const restoredCompactionCount = (
  { resetEffort, resetInterval, compactionCount }: PersistedState,
  { effort, interval }: ResetPolicy,
): number => (resetEffort === effort && resetInterval === interval ? (compactionCount ?? 0) : 0);

function clearedOverride(
  overrides: SessionOverrides,
  key: keyof SessionOverrides,
): SessionOverrides {
  if (key === "startEffort") {
    return { ...overrides, startEffort: undefined };
  }
  if (key === "endEffort") {
    return { ...overrides, endEffort: undefined };
  }
  if (key === "compactionEffort") {
    return { ...overrides, compactionEffort: undefined };
  }
  return key === "compactionResetEffort"
    ? { ...overrides, compactionResetEffort: undefined }
    : { ...overrides, compactionResetInterval: undefined };
}

export function setSessionOverride(
  current: SessionOverrides,
  key: keyof SessionOverrides,
  value: string,
): SessionOverrides | string {
  const overrides: SessionOverrides = { ...current };
  if (value === "default") {
    return clearedOverride(overrides, key);
  }
  if (key === "compactionResetInterval") {
    const interval = Number(value);
    if (!Number.isSafeInteger(interval) || interval <= 0) {
      return "Compaction reset interval must be a positive integer.";
    }
    overrides.compactionResetInterval = interval;
    return overrides;
  }
  const effort = thinkingLevel(value);
  if (effort === undefined) {
    return `Invalid effort level: ${value}.`;
  }
  overrides[key] = effort;
  return overrides;
}

/** Counts a compaction that ran at or above the reset effort; every `interval`-th one resets. */
export function countCompaction({
  level,
  count,
  effort,
  interval,
}: CompactionCount): CompactionOutcome {
  const qualifies = effortAtLeast(level, effort);
  const counted = count + Number(qualifies);
  return { count: counted, reset: qualifies && shouldResetAfterCompaction(counted, interval) };
}
