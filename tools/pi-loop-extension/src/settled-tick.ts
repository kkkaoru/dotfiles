// This TypeScript file is executed with Bun.
import type { LoopHost } from "./contracts.ts";
import { trySendUserMessage } from "./helpers.ts";

export const ABANDONED_LOOP_NOTICE =
  "Loop stopped: tick ended without loop_wakeup or loop_complete.";
export const CONTINUED_LOOP_NOTICE = "Continuing unfinished loop work.";

export interface SettledTickState {
  continuedWithoutTerminal: boolean;
  jobs: number;
  pending: readonly string[];
  running: string | undefined;
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
  if (state.continuedWithoutTerminal) {
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
