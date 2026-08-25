// This TypeScript file is executed with Bun.
import { expect, it } from "vitest";
import { resumedJob } from "./helpers.ts";

it("resumes one-shot and recurring jobs with or without a remaining delay", () => {
  const common = {
    id: 1,
    nextRunAt: 10,
    prompt: "inspect",
    reason: "wait",
    submittedAt: 5,
  };
  expect(resumedJob(common, 100).nextRunAt).toBe(100);
  expect(resumedJob({ ...common, intervalMs: 60, remainingMs: 20 }, 100)).toEqual({
    ...common,
    intervalMs: 60,
    nextRunAt: 120,
  });
});
