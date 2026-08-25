// This TypeScript file is executed with Bun.
import { expect, it } from "vitest";
import { namedLoopFollowUp } from "./follow-up.ts";

it("names loop follow-ups with local submission and completion times", () => {
  expect(
    namedLoopFollowUp({
      completedAt: new Date(2026, 7, 26, 1, 15).getTime(),
      identity: "  #3 | Inspect   queued\nresults  ",
      prompt: "Inspect /tmp/result.log and continue.",
      submittedAt: new Date(2026, 7, 26, 1, 5).getTime(),
    }),
  ).toBe(
    "08-26 01:05 → 01:15 | loop=#3 | Inspect queued results\nInspect /tmp/result.log and continue.",
  );
});

it("bounds long follow-up identities", () => {
  expect(
    namedLoopFollowUp({
      completedAt: new Date(2026, 7, 26, 1, 15).getTime(),
      identity: "x".repeat(200),
      prompt: "Continue.",
      submittedAt: new Date(2026, 7, 26, 1, 5).getTime(),
    }).split("\n")[0],
  ).toHaveLength(147);
});
