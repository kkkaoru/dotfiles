// This TypeScript file is executed with Bun.
import type { LoopHost } from "./contracts.ts";
import { trySendUserMessage } from "./helpers.ts";

export const ABANDONED_LOOP_NOTICE =
  "Loop stopped: tick ended without loop_wakeup or loop_complete.";
export const CONTINUED_LOOP_NOTICE = "Continuing unfinished loop work.";
/** Consecutive auto-continues while watched detached work stays unfinished. */
export const MAX_AUTO_EXTEND = 10;

export interface SettledTickState {
  continuedWithoutTerminal: boolean;
  jobs: number;
  pending: readonly string[];
  running: string | undefined;
  /** Loop-owned detached work is still live or undelivered; never abandon it silently. */
  unfinishedWork: boolean;
}

export interface SettledTickActions {
  abandon: () => void;
  clearPending: () => void;
  clearRunning: () => void;
  markContinued: () => void;
  notify: (message: string, level: "info" | "warning") => void;
  persist: () => void;
  queue: (text: string) => void;
  updateStatus: () => void;
}

/** Bounded auto-extends for a tick with unfinished detached work. */
export class AutoExtendBudget {
  #used = 0;

  /**
   * True while the tick may continue instead of being abandoned. A fresh
   * settle starts a new decision cycle and resets the count.
   */
  extend(continued: boolean, unfinishedWork: boolean): boolean {
    if (!continued) {
      this.#used = 0;
    }
    if (!unfinishedWork || this.#used >= MAX_AUTO_EXTEND) {
      return false;
    }
    if (continued) {
      this.#used += 1;
    }
    return true;
  }
}

function deliverPending(
  pending: readonly string[],
  actions: SettledTickActions,
  host: LoopHost,
): void {
  if (trySendUserMessage(host, pending.join("\n\n"))) {
    actions.clearPending();
    actions.persist();
    actions.updateStatus();
  }
}

function continueRunning(
  state: SettledTickState,
  actions: SettledTickActions,
  host: LoopHost,
): void {
  if (state.running === undefined) {
    return;
  }
  if (state.jobs > 0) {
    actions.clearRunning();
    actions.persist();
    return;
  }
  if (state.continuedWithoutTerminal && !state.unfinishedWork) {
    actions.abandon();
    return;
  }
  if (!trySendUserMessage(host, state.running)) {
    actions.queue(state.running);
    return;
  }
  actions.markContinued();
  actions.notify(CONTINUED_LOOP_NOTICE, "info");
  actions.persist();
}

export function settleTick(
  state: SettledTickState,
  actions: SettledTickActions,
  host: LoopHost,
): void {
  if (state.pending.length > 0) {
    deliverPending(state.pending, actions, host);
    return;
  }
  continueRunning(state, actions, host);
}
