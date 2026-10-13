// This TypeScript file is executed with Bun.
import type { ThinkingLevel } from "@earendil-works/pi-ai";
import type { EffortProfile } from "./policy.ts";
import { isRecord } from "./settings.ts";

export interface RecoveryState {
  readonly steps: number;
  readonly successes: number;
  readonly lastTurn: string;
}
export interface TurnEvidence {
  readonly id: string;
  readonly results: readonly { readonly isError: boolean }[];
}
const MAX_STEPS = 6 satisfies number;
const RECOVERY_ROUNDS = 2 satisfies number;
const EMPTY: RecoveryState = { steps: 0, successes: 0, lastTurn: "" };

function boundedInteger(value: unknown, maximum: number): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 && value <= maximum;
}

export function readRecoveryState(value: unknown): RecoveryState {
  if (!isRecord(value) || typeof value["lastTurn"] !== "string") {
    return { ...EMPTY };
  }
  const steps: unknown = value["steps"];
  const successes: unknown = value["successes"];
  return boundedInteger(steps, MAX_STEPS) && boundedInteger(successes, RECOVERY_ROUNDS - 1)
    ? { steps, successes, lastTurn: value["lastTurn"] }
    : { ...EMPTY };
}

/** Evidence changes effort, never authorization, completion, or continuation ownership. */
export class RecoveryEffort {
  #state: RecoveryState = { ...EMPTY };

  restore(value: unknown): void {
    this.#state = readRecoveryState(value);
  }
  snapshot(): RecoveryState {
    return { ...this.#state };
  }

  observe(evidence: TurnEvidence): boolean {
    if (
      evidence.id === "" ||
      evidence.id === this.#state.lastTurn ||
      evidence.results.length === 0
    ) {
      return false;
    }
    const failed: boolean = evidence.results.some((result) => result.isError);
    const successes: number = failed ? 0 : this.#state.successes + 1;
    const recovered: boolean = successes >= RECOVERY_ROUNDS;
    this.#state = {
      lastTurn: evidence.id,
      steps: failed
        ? Math.min(MAX_STEPS, this.#state.steps + 1)
        : Math.max(0, this.#state.steps - Number(recovered)),
      successes: recovered ? 0 : successes,
    };
    return true;
  }

  select(profile: EffortProfile, selected: ThinkingLevel): ThinkingLevel {
    const index: number = Math.max(
      profile.operational.indexOf(selected),
      Math.min(this.#state.steps, profile.operational.length - 1),
    );
    return profile.operational[index] ?? selected;
  }
}
