// This TypeScript file is executed with Bun.
import type { Timeline } from "./tree.ts";

export const NUDGE_TYPE = "timeline-nudge";
/**
 * Tool calls without a timeline call before the agent is told to record progress, the reference
 * package's default iteration count. A turn can run for an hour, so the notice must reach it
 * mid-turn rather than at the end of one.
 */
export const STEPS_BEFORE_NUDGE = 24;
/** How often the head's age is brought up to date, since only a change would repaint it. */
export const TICK_MS = 60_000;

/** What a session knows about its own tree. */
export interface Tracked {
  /** Tool calls since the last timeline call. */
  steps: number;
  readonly timeline: Timeline;
  updatedAt: number;
}

/** How long ago the tree changed, which is what the head says. */
export function ageOf(tracked: { readonly updatedAt: number }): number {
  return Date.now() - tracked.updatedAt;
}

/** Repaint on a timer, since nothing else would move the age; the timer never outlives pi. */
export function everyTick(paint: () => void): () => void {
  const timer = globalThis.setInterval(paint, TICK_MS);
  (timer as { unref?: () => void }).unref?.();
  return () => {
    globalThis.clearInterval(timer);
  };
}

/**
 * The checkpoint a long stretch of work earns, or undefined while the tree is fresh or finished.
 * The count restarts either way, so the next stretch is reported on its own.
 */
export function checkpoint(tracked: Tracked): string | undefined {
  if (tracked.steps < STEPS_BEFORE_NUDGE) {
    return undefined;
  }
  tracked.steps = 0;
  const unfinished = tracked.timeline.tasks.filter((task) => task.status !== "done").length;
  if (unfinished === 0) {
    return undefined;
  }
  return `Task progress checkpoint: ${String(unfinished)} unfinished. Save this goal's tree with the timeline tool — mark what finished as done, keep the current task active with a short note of its result, and add tasks if the scope changed. Use action "list" when choosing the next task.`;
}
