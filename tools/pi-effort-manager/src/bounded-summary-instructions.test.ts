// This TypeScript file is executed with Bun.
import { expect, it } from "vitest";
import { guidedInstructions } from "./bounded-summary.ts";

it("adds the abandoned-approaches guideline after the person's compaction focus", () => {
  expect(guidedInstructions(undefined)).toBe(
    "Under Critical Context, also list approaches that were tried and abandoned, with the error or reason, so they are not repeated.",
  );
  expect(guidedInstructions("Keep next steps")).toBe(
    "Keep next steps\n\nUnder Critical Context, also list approaches that were tried and abandoned, with the error or reason, so they are not repeated.",
  );
});
