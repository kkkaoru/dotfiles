// This TypeScript file is executed with Bun.
import { loopListText } from "./display.ts";
import { resumedJob } from "./helpers.ts";
import type { LoopJobState as LoopJob } from "./state.ts";

interface PauseInput {
  readonly jobs: ReadonlyMap<number, LoopJob>;
  readonly now: number;
}

interface ResumeInput {
  readonly jobs: ReadonlyMap<number, LoopJob>;
  readonly now: number;
}

interface ListInput {
  readonly jobs: ReadonlyMap<number, LoopJob>;
  readonly now: number;
  readonly paused: boolean;
}

export function pauseJobs(input: PauseInput): Map<number, LoopJob> {
  return new Map(
    [...input.jobs.entries()].map(
      ([id, job]: readonly [number, LoopJob]): readonly [number, LoopJob] => [
        id,
        { ...job, remainingMs: Math.max(0, job.nextRunAt - input.now) },
      ],
    ),
  );
}

export function resumeJobs(input: ResumeInput): Map<number, LoopJob> {
  return new Map(
    [...input.jobs.entries()].map(
      ([id, job]: readonly [number, LoopJob]): readonly [number, LoopJob] => [
        id,
        resumedJob(job, input.now),
      ],
    ),
  );
}

export function loopListMessage(input: ListInput): string {
  return input.jobs.size === 0
    ? "No loop jobs are scheduled."
    : loopListText([...input.jobs.values()], input.paused, input.now);
}
