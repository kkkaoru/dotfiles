import type { ThinkingLevel } from "@earendil-works/pi-ai";

export const STATE_ENTRY_TYPE = "pi-effort-manager-state-v1";

const THINKING_LEVELS = new Set<ThinkingLevel>([
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
]);

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

export interface ResetPolicyDefaults {
  compactionResetEffort: ThinkingLevel;
  compactionResetInterval: number;
}

export interface PersistedState {
  compactionCount?: number | undefined;
  enabled?: boolean | undefined;
  overrides?: SessionOverrides | undefined;
  resetEffort?: ThinkingLevel | undefined;
  resetInterval?: number | undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function thinkingLevel(value: unknown): ThinkingLevel | undefined {
  return typeof value === "string" && THINKING_LEVELS.has(value as ThinkingLevel)
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
    enabled: typeof data["enabled"] === "boolean" ? data["enabled"] : undefined,
    overrides: sessionOverrides(data["overrides"]),
    resetEffort: thinkingLevel(data["resetEffort"]),
    resetInterval: positiveInteger(data["resetInterval"]),
  };
}

export function latestPersistedState(entries: readonly unknown[]): PersistedState {
  const latest = entries.findLast((entry): boolean => persistedState(entry) !== undefined);
  return persistedState(latest) ?? {};
}

export function effectiveResetPolicy(
  overrides: SessionOverrides,
  defaults: ResetPolicyDefaults,
): ResetPolicy {
  return {
    effort: overrides.compactionResetEffort ?? defaults.compactionResetEffort,
    interval: overrides.compactionResetInterval ?? defaults.compactionResetInterval,
  };
}

export function restoredCompactionCount(persisted: PersistedState, policy: ResetPolicy): number {
  const matches =
    persisted.resetEffort === policy.effort && persisted.resetInterval === policy.interval;
  return matches ? (persisted.compactionCount ?? 0) : 0;
}

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
