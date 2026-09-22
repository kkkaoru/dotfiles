// This TypeScript file is executed with Bun.
import {
  queryActivity,
  subscribeTasks,
  type ActivityBus,
  type ActivitySnapshot,
  type TaskNotice,
} from "./goal-activity.ts";

/** Task names still live or awaiting delivery across activity snapshots. */
export function liveTaskNames(snapshots: readonly ActivitySnapshot[]): Set<string> {
  const live = new Set<string>();
  for (const snapshot of snapshots) {
    for (const name of snapshot.tasks) {
      live.add(name);
    }
    for (const name of snapshot.pendingTasks ?? []) {
      live.add(name);
    }
  }
  return live;
}

/** Drop watched names that are no longer live or awaiting delivery. */
export function pruneWatchedTasks(
  watched: ReadonlySet<string>,
  snapshots: readonly ActivitySnapshot[],
): Set<string> {
  const live: Set<string> = liveTaskNames(snapshots);
  const pruned = new Set<string>();
  for (const name of watched) {
    if (live.has(name)) {
      pruned.add(name);
    }
  }
  return pruned;
}

/**
 * Detached launches announced while a loop tick is in flight. A tick must not be abandoned while
 * its own launches are still running or their completion notice is undelivered.
 */
export class WatchedLoopTasks {
  #names = new Set<string>();
  #removeSubscription: (() => void) | undefined;
  #sessionId: string | undefined;

  attach(
    bus: ActivityBus | undefined,
    sessionId: string | undefined,
    isActive: () => boolean,
  ): void {
    this.detach();
    this.#sessionId = sessionId;
    if (bus === undefined || sessionId === undefined) {
      return;
    }
    const listener = (notice: TaskNotice): void => {
      if (notice.sessionId === sessionId && isActive()) {
        this.#names.add(notice.name);
      }
    };
    this.#removeSubscription = subscribeTasks(bus, listener);
  }

  detach(): void {
    this.#removeSubscription?.();
    this.#removeSubscription = undefined;
  }

  clear(): void {
    this.#names.clear();
  }

  /** Prune finished work and report whether watched launches are still unfinished. */
  unfinished(bus: ActivityBus | undefined): boolean {
    if (bus === undefined || this.#sessionId === undefined || this.#names.size === 0) {
      return false;
    }
    this.#names = pruneWatchedTasks(this.#names, queryActivity(bus, this.#sessionId));
    return this.#names.size > 0;
  }
}
