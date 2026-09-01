// This TypeScript file is executed with Bun.
import { expect, it } from "vitest";
import { loopListMessage, pauseJobs, resumeJobs } from "./job-control.ts";
import type { LoopJobState } from "./state.ts";

const JOB: LoopJobState = {
  id: 1,
  nextRunAt: 61_000,
  prompt: "check CI",
  reason: "pending",
  submittedAt: 1000,
};

it("pauses and resumes loop jobs from their remaining delay", () => {
  const paused = pauseJobs({ jobs: new Map([[1, JOB]]), now: 31_000 });
  const resumed = resumeJobs({ jobs: paused, now: 101_000 });

  expect(paused.get(1)?.remainingMs).toBe(30_000);
  expect(resumed.get(1)).toStrictEqual({
    id: 1,
    nextRunAt: 131_000,
    prompt: "check CI",
    reason: "pending",
    submittedAt: 1000,
  });
});

it("formats empty and scheduled loop lists", () => {
  expect(loopListMessage({ jobs: new Map(), now: 1000, paused: false })).toBe(
    "No loop jobs are scheduled.",
  );
  expect(loopListMessage({ jobs: new Map([[1, JOB]]), now: 1000, paused: false })).toBe(
    "#1 in 1m: pending",
  );
});
