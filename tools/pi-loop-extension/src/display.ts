// This TypeScript file is executed with Bun.
import { namedLoopSchedule } from "./follow-up.ts";
import { formatInterval } from "./parser.ts";

export interface LoopDisplayJob {
  readonly id: number;
  readonly nextRunAt: number;
  readonly reason: string;
  readonly remainingMs?: number;
  readonly submittedAt: number;
}

export function loopListText(
  jobs: readonly LoopDisplayJob[],
  paused: boolean,
  now: number,
): string {
  return jobs
    .map((job: LoopDisplayJob): string => {
      const remainingMs: number = job.remainingMs ?? Math.max(0, job.nextRunAt - now);
      const state: string = paused ? "paused, " : "";
      return `#${String(job.id)} ${state}in ${formatInterval(remainingMs)}: ${job.reason}`;
    })
    .join("\n");
}

export function loopStatus(
  jobCount: number,
  pendingCount: number,
  paused: boolean,
): string | undefined {
  if (jobCount === 0 && pendingCount === 0) {
    return undefined;
  }
  const pausedLabel: string = paused ? " (paused)" : "";
  const ready: string = pendingCount === 0 ? "" : `, ready: ${String(pendingCount)}`;
  return `loop: ${String(jobCount)}${pausedLabel}${ready}`;
}

export interface LoopDisplayUi {
  readonly setStatus: (key: string, value: string | undefined) => void;
  readonly setWidget?: (key: string, lines: readonly string[] | undefined) => void;
}

export function loopWidgetLines(
  jobs: readonly LoopDisplayJob[],
  pendingContinuations: readonly string[],
  now: number,
): readonly string[] {
  const scheduled: readonly string[] = jobs.map((job: LoopDisplayJob): string =>
    namedLoopSchedule({
      identity: `#${String(job.id)} | ${job.reason}`,
      scheduledAt: job.remainingMs === undefined ? job.nextRunAt : now + job.remainingMs,
      submittedAt: job.submittedAt,
    }),
  );
  const pending: readonly string[] = pendingContinuations.map((continuation: string): string =>
    continuation.replace(/\n[\s\S]*$/u, ""),
  );
  return [...scheduled, ...pending];
}

export function clearLoopDisplay(ui: LoopDisplayUi | undefined): void {
  ui?.setStatus("loop", undefined);
  ui?.setWidget?.("loop-wakeups", undefined);
}

export function updateLoopDisplay(input: {
  readonly jobs: readonly LoopDisplayJob[];
  readonly now: number;
  readonly paused: boolean;
  readonly pendingContinuations: readonly string[];
  readonly ui: LoopDisplayUi | undefined;
}): void {
  const status: string | undefined = loopStatus(
    input.jobs.length,
    input.pendingContinuations.length,
    input.paused,
  );
  const widget: readonly string[] = loopWidgetLines(
    input.jobs,
    input.pendingContinuations,
    input.now,
  );
  input.ui?.setStatus("loop", status);
  input.ui?.setWidget?.("loop-wakeups", widget.length === 0 ? undefined : widget);
}
