// This TypeScript file is executed with Bun.
import {
  ACTIVE_DISPLAY_ENTRY_TYPE,
  type ActiveTaskDisplay,
  recoverActiveTaskDisplayState,
} from "./active-display.ts";
import type { CompletionDelivery, CompletionDeliveryContext } from "./delivery.ts";
import { recoverSessionTmuxLaunches, type RecoveryOptions } from "./persistence.ts";
import type { TmuxLaunch, TmuxRuntime } from "./tmux.ts";

interface DisplaySync {
  readonly display: ActiveTaskDisplay;
  readonly delivery: CompletionDelivery;
  readonly runtime: TmuxRuntime;
  readonly persist: ((type: string, data: unknown) => void) | undefined;
}
interface RestoreSync extends Pick<DisplaySync, "display" | "delivery" | "runtime"> {
  readonly context: CompletionDeliveryContext;
  readonly recovery: false | Omit<RecoveryOptions, "sessionNamespace"> | undefined;
  readonly sessionManager: NonNullable<CompletionDeliveryContext["sessionManager"]>;
}

export function inspectOverdue(event: unknown, sync: DisplaySync): void {
  const now = Date.now();
  const inspected = sync.display.acknowledge(sync.delivery.inspectedLog(event), now);
  if (inspected.length === 0) {
    return;
  }
  sync.runtime.acknowledgeOverdue(inspected, now);
  sync.persist?.(ACTIVE_DISPLAY_ENTRY_TYPE, sync.display.state());
}

export function remindOverdue(launches: readonly TmuxLaunch[], sync: DisplaySync): void {
  if (sync.display.remind(launches)) {
    sync.persist?.(ACTIVE_DISPLAY_ENTRY_TYPE, sync.display.state());
  }
  sync.delivery.overdue(launches);
}

export function restoreOverdue(sync: RestoreSync): void {
  const state = recoverActiveTaskDisplayState(sync.sessionManager.getEntries());
  sync.display.restore(state);
  sync.display.setContext(sync.context);
  sync.delivery.setContext(sync.context);
  sync.delivery.beforeCompaction();
  const namespace = sync.runtime.startSession(sync.sessionManager.getSessionId());
  sync.runtime.restore(
    recoverSessionTmuxLaunches(
      sync.sessionManager.getEntries(),
      namespace,
      sync.recovery === false ? undefined : sync.recovery?.operations,
    ),
    1,
    state.acknowledgedAt,
  );
  sync.delivery.deferAfterCompaction(sync.context);
}
