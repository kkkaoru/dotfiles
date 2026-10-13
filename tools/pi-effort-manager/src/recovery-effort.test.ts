// This TypeScript file is executed with Bun.
import { expect, it } from "vitest";
import { RecoveryEffort, readRecoveryState } from "./recovery-effort.ts";
import type { EffortProfile } from "./policy.ts";
const PROFILE: EffortProfile = {
  baseline: "medium",
  compaction: "max",
  operational: ["medium", "high", "xhigh"],
  supported: ["medium", "high", "xhigh", "max"],
};
it("raises effort per failed round, deduplicates evidence and caps at the work boundary", () => {
  const recovery = new RecoveryEffort();
  expect(recovery.observe({ id: "", results: [] })).toBe(false);
  expect(recovery.observe({ id: "empty", results: [] })).toBe(false);
  expect(recovery.observe({ id: "one", results: [{ isError: true }, { isError: true }] })).toBe(
    true,
  );
  expect(recovery.select(PROFILE, "medium")).toBe("high");
  expect(recovery.observe({ id: "one", results: [{ isError: true }] })).toBe(false);
  recovery.observe({ id: "two", results: [{ isError: true }] });
  expect(recovery.select(PROFILE, "medium")).toBe("xhigh");
  expect(recovery.select(PROFILE, "xhigh")).toBe("xhigh");
  recovery.restore({ steps: 6, successes: 0, lastTurn: "old" });
  recovery.observe({ id: "new", results: [{ isError: true }] });
  expect(recovery.snapshot()).toStrictEqual({ steps: 6, successes: 0, lastTurn: "new" });
});
it("requires two clean tool rounds before reducing effort and never drops a context floor", () => {
  const recovery = new RecoveryEffort();
  recovery.restore({ steps: 1, successes: 0, lastTurn: "failed" });
  recovery.observe({ id: "ok-1", results: [{ isError: false }] });
  expect(recovery.select(PROFILE, "medium")).toBe("high");
  recovery.observe({ id: "ok-2", results: [{ isError: false }] });
  expect(recovery.select(PROFILE, "medium")).toBe("medium");
  expect(recovery.select(PROFILE, "xhigh")).toBe("xhigh");
  expect(recovery.select({ ...PROFILE, operational: [] }, "medium")).toBe("medium");
});
it.each([
  null,
  {},
  { lastTurn: "x", steps: -1, successes: 0 },
  { lastTurn: "x", steps: 7, successes: 0 },
  { lastTurn: "x", steps: 1.5, successes: 0 },
  { lastTurn: "x", steps: "1", successes: 0 },
  { lastTurn: "x", steps: 1, successes: -1 },
  { lastTurn: "x", steps: 1, successes: 2 },
  { lastTurn: "x", steps: 1, successes: "0" },
])("rejects malformed recovery evidence %j", (value) => {
  expect(readRecoveryState(value)).toStrictEqual({ steps: 0, successes: 0, lastTurn: "" });
});
