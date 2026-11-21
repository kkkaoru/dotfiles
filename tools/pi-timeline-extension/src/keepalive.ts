// This TypeScript file is executed with Bun.
import { ageText, type Timeline } from "./tree.ts";

export const NUDGE_TYPE = "timeline-nudge";
/** A tree untouched this long while work continues is the one that misleads, so the agent is told. */
export const NUDGE_AFTER_MS = 15 * 60_000;
/** How often the head's age is brought up to date, since only a change would repaint it. */
export const TICK_MS = 60_000;

/** What a session knows about its own tree. */
export interface Tracked {
  nudgedAt: number | undefined;
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

/** The reminder a stale tree earns, or undefined when it is current, finished, or already reported. */
export function staleNudge(tracked: Tracked, now: number): string | undefined {
  const unfinished = tracked.timeline.tasks.filter((task) => task.status !== "done").length;
  const age = now - tracked.updatedAt;
  if (unfinished === 0 || age < NUDGE_AFTER_MS) {
    return undefined;
  }
  // One reminder per untouched tree: a change clears it, so the next stale stretch is reported.
  if (tracked.nudgedAt !== undefined && tracked.nudgedAt >= tracked.updatedAt) {
    return undefined;
  }
  tracked.nudgedAt = now;
  return `Timeline: ${String(unfinished)} unfinished and the tree has not changed for ${ageText(age)}. Mark what finished as done and record the next step with the timeline tool.`;
}
