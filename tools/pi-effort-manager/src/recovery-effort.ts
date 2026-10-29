// This TypeScript file is executed with Bun.
import type { ThinkingLevel } from "@earendil-works/pi-ai";
import type { EffortProfile } from "./policy.ts";
import { isRecord } from "./settings.ts";
import { branchResults } from "./tool-signature.ts";

export interface RecoveryState {
  readonly steps: number;
  readonly successes: number;
  readonly lastTurn: string;
}
/** One tool result of a round; `signature` identifies its input and outcome (tool-signature.ts). */
export interface RoundResult {
  readonly isError: boolean;
  readonly malformed?: boolean | undefined;
  readonly inputKey?: string | undefined;
  readonly signature?: string | undefined;
}
interface RoundOutcome {
  readonly id: string;
  readonly failed: boolean;
  readonly held: boolean;
  readonly verified: boolean;
  readonly cap: number;
}
export interface TurnEvidence {
  readonly id: string;
  readonly results: readonly RoundResult[];
}
const MAX_STEPS = 6 satisfies number;
const RECOVERY_ROUNDS = 2 satisfies number;
// A call repeating one of the last few calls with the same outcome made no progress.
// Step repetition (MAST, arXiv:2503.13657) therefore counts like a failure.
const REPEAT_WINDOW = 8 satisfies number;
const EMPTY: RecoveryState = { steps: 0, successes: 0, lastTurn: "" };
// After this many failing rounds in a row at the work cap, more reasoning on one path stops paying.
// See Mirage of test-time scaling (arXiv:2506.04210) and SWE-Effi (arXiv:2509.09853).
const STUCK_ROUNDS = 3 satisfies number;
export const STUCK_NOTE =
  "pi-effort-manager: this approach has now failed for several rounds at the highest work effort. More reasoning on the same path is unlikely to help: state what is blocking, then try a different approach or ask the person." satisfies string;

const isFailure = ({ isError, malformed }: RoundResult): boolean => isError && malformed !== true;

/** A fixed-size ring of recent keys: constant memory, constant-time membership. Not persisted. */
interface Ring {
  readonly items: string[];
  cursor: number;
}

const emptyRing = (): Ring => ({
  items: Array.from({ length: REPEAT_WINDOW }, () => ""),
  cursor: 0,
});

/** Adds a key and reports whether it was already among the recent ones. */
function pushRing(ring: Ring, key: string): boolean {
  const seen = ring.items.includes(key);
  ring.items[ring.cursor] = key;
  ring.cursor = (ring.cursor + 1) % REPEAT_WINDOW;
  return seen;
}

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
  #recent: Ring = emptyRing();
  // Inputs of recent failed calls: the same input succeeding later is a verified fix.
  #failedInputs: Ring = emptyRing();
  #stuck = 0;
  #nudged = false;

  /** Restores the persisted floor and refills both windows from the branch's tool results. */
  restore(value: unknown, entries: readonly unknown[]): void {
    this.#state = readRecoveryState(value);
    this.#recent = emptyRing();
    this.#failedInputs = emptyRing();
    for (const result of branchResults(entries)) {
      this.#remember(result);
    }
  }
  snapshot(): RecoveryState {
    return { ...this.#state };
  }

  /**
   * Folds a round into the floor and reports whether the floor moved (the only change worth
   * persisting). A failure raises it; a repeat with the same outcome or a malformed call holds it;
   * a verified fix (a failed input now succeeding: DEER, arXiv:2504.15895; Self-Debugging,
   * arXiv:2304.05128) lowers it at once; two clean rounds lower it. The floor never exceeds what the
   * profile's work levels can express: holding a raised level after recovery is overthinking
   * (arXiv:2502.08235) and can lower accuracy (inverse scaling, arXiv:2507.14417). Without a
   * reasoning profile nothing counts.
   */
  observe({ id, results }: TurnEvidence, profile: EffortProfile | undefined): boolean {
    if (profile === undefined || id === "" || id === this.#state.lastTurn || results.length === 0) {
      return false;
    }
    const failed: boolean = results.some((result) => isFailure(result));
    // Read before #remember records this round's failures, so a failure cannot verify itself.
    const verified: boolean =
      !failed &&
      results.some(
        ({ isError, inputKey }) =>
          !isError && inputKey !== undefined && this.#failedInputs.items.includes(inputKey),
      );
    const held: boolean =
      this.#rememberAll(results) || results.some(({ malformed }) => malformed === true);
    return this.#settle({
      id,
      failed,
      held,
      verified,
      cap: Math.min(MAX_STEPS, profile.operational.length - 1),
    });
  }

  /** Claims the one change-approach note for a failure that extends a run of failures at the cap. */
  claimStuckNote(result: RoundResult, profile: EffortProfile | undefined): boolean {
    const stuck: boolean =
      profile !== undefined &&
      isFailure(result) &&
      this.#state.steps >= Math.min(MAX_STEPS, profile.operational.length - 1) &&
      this.#stuck >= STUCK_ROUNDS - 1 &&
      !this.#nudged;
    this.#nudged ||= stuck;
    return stuck;
  }

  #settle({ id, failed, held, verified, cap }: RoundOutcome): boolean {
    const successes: number = failed || held || verified ? 0 : this.#state.successes + 1;
    const recovered: boolean = successes >= RECOVERY_ROUNDS;
    const steps: number = Math.min(
      cap,
      failed
        ? this.#state.steps + 1
        : Math.max(0, this.#state.steps - Number(recovered || verified)),
    );
    this.#stuck = failed && this.#state.steps >= cap ? this.#stuck + 1 : 0;
    this.#nudged = failed && this.#nudged;
    const moved: boolean = steps !== this.#state.steps;
    this.#state = { lastTurn: id, steps, successes: recovered ? 0 : successes };
    return moved;
  }

  /** Remembers every result, without an intermediate array, and reports whether any repeated. */
  #rememberAll(results: readonly RoundResult[]): boolean {
    let repeated = false;
    for (const result of results) {
      repeated = this.#remember(result) || repeated;
    }
    return repeated;
  }

  /** Remembers a result and reports whether it repeats a recent call with the same outcome. */
  #remember(result: RoundResult): boolean {
    if (isFailure(result) && result.inputKey !== undefined) {
      pushRing(this.#failedInputs, result.inputKey);
    }
    return result.signature !== undefined && pushRing(this.#recent, result.signature);
  }

  select(profile: EffortProfile, selected: ThinkingLevel): ThinkingLevel {
    return (
      profile.operational[
        Math.max(
          profile.operational.indexOf(selected),
          Math.min(this.#state.steps, profile.operational.length - 1),
        )
      ] ?? selected
    );
  }
}
