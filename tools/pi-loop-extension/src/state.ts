// This TypeScript file is executed with Bun.

export const LOOP_STATE_ENTRY_TYPE = "pi-loop-state-v1";

export interface LoopJobState {
  readonly id: number;
  readonly intervalMs?: number;
  readonly nextRunAt: number;
  readonly prompt: string;
  readonly reason: string;
  readonly remainingMs?: number;
  readonly submittedAt: number;
}

export interface LoopRuntimeState {
  readonly jobs: readonly LoopJobState[];
  readonly nextId: number;
  readonly paused: boolean;
  readonly pendingContinuations: readonly string[];
  readonly runningContinuation?: string;
  readonly version: 1;
}

function finiteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

function optionalFiniteNumber(value: unknown): boolean {
  return value === undefined || finiteNumber(value);
}

function validJob(value: unknown): value is LoopJobState {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const job = value as Record<string, unknown>;
  return (
    Number.isInteger(job["id"]) &&
    finiteNumber(job["nextRunAt"]) &&
    finiteNumber(job["submittedAt"]) &&
    typeof job["prompt"] === "string" &&
    typeof job["reason"] === "string" &&
    optionalFiniteNumber(job["intervalMs"]) &&
    optionalFiniteNumber(job["remainingMs"])
  );
}

function validRunningContinuation(value: unknown): boolean {
  return value === undefined || typeof value === "string";
}

function validState(value: unknown): value is LoopRuntimeState {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const state = value as Record<string, unknown>;
  const validJobs: boolean = Array.isArray(state["jobs"]) && state["jobs"].every(validJob);
  const validPending: boolean =
    Array.isArray(state["pendingContinuations"]) &&
    state["pendingContinuations"].every(
      (item: unknown): item is string => typeof item === "string",
    );
  return (
    state["version"] === 1 &&
    validJobs &&
    Number.isInteger(state["nextId"]) &&
    typeof state["paused"] === "boolean" &&
    validPending &&
    validRunningContinuation(state["runningContinuation"])
  );
}

export function restoredPendingContinuations(state: LoopRuntimeState): readonly string[] {
  const pending: string[] = [...state.pendingContinuations];
  if (
    state.runningContinuation !== undefined &&
    state.jobs.length === 0 &&
    !pending.includes(state.runningContinuation)
  ) {
    pending.push(state.runningContinuation);
  }
  return pending;
}

export function createLoopState(input: {
  readonly jobs: readonly LoopJobState[];
  readonly nextId: number;
  readonly paused: boolean;
  readonly pendingContinuations: readonly string[];
  readonly runningContinuation: string | undefined;
}): LoopRuntimeState {
  const common = {
    jobs: input.jobs,
    nextId: input.nextId,
    paused: input.paused,
    pendingContinuations: input.pendingContinuations,
    version: 1 as const,
  };
  return input.runningContinuation === undefined
    ? common
    : { ...common, runningContinuation: input.runningContinuation };
}

export function persistLoopState(
  writer: ((customType: string, data: unknown) => void) | undefined,
  input: Parameters<typeof createLoopState>[0],
): void {
  writer?.(LOOP_STATE_ENTRY_TYPE, createLoopState(input));
}

export function latestLoopState(entries: readonly unknown[]): LoopRuntimeState | undefined {
  for (let index = entries.length - 1; index >= 0; index -= 1) {
    const entry: unknown = entries[index];
    if (
      typeof entry === "object" &&
      entry !== null &&
      "type" in entry &&
      entry.type === "custom" &&
      "customType" in entry &&
      entry.customType === LOOP_STATE_ENTRY_TYPE &&
      "data" in entry &&
      validState(entry.data)
    ) {
      return entry.data;
    }
  }
  return undefined;
}
