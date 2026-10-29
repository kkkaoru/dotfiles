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
  expect(recovery.observe({ id: "", results: [] }, PROFILE)).toBe(false);
  expect(recovery.observe({ id: "empty", results: [] }, PROFILE)).toBe(false);
  expect(
    recovery.observe({ id: "one", results: [{ isError: true }, { isError: true }] }, PROFILE),
  ).toBe(true);
  expect(recovery.select(PROFILE, "medium")).toBe("high");
  expect(recovery.observe({ id: "one", results: [{ isError: true }] }, PROFILE)).toBe(false);
  recovery.observe({ id: "two", results: [{ isError: true }] }, PROFILE);
  expect(recovery.select(PROFILE, "medium")).toBe("xhigh");
  expect(recovery.select(PROFILE, "xhigh")).toBe("xhigh");
  recovery.restore({ steps: 6, successes: 0, lastTurn: "old" }, []);
  recovery.observe({ id: "new", results: [{ isError: true }] }, PROFILE);
  expect(recovery.snapshot()).toStrictEqual({ steps: 2, successes: 0, lastTurn: "new" });
});
it("requires two clean tool rounds before reducing effort and never drops a context floor", () => {
  const recovery = new RecoveryEffort();
  recovery.restore({ steps: 1, successes: 0, lastTurn: "failed" }, []);
  recovery.observe({ id: "ok-1", results: [{ isError: false }] }, PROFILE);
  expect(recovery.select(PROFILE, "medium")).toBe("high");
  recovery.observe({ id: "ok-2", results: [{ isError: false }] }, PROFILE);
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
it("holds the floor on a call repeated with the same outcome instead of counting a success", () => {
  const recovery = new RecoveryEffort();
  recovery.observe({ id: "first", results: [{ isError: false, signature: "test:FAIL" }] }, PROFILE);
  expect(recovery.snapshot()).toStrictEqual({ steps: 0, successes: 1, lastTurn: "first" });
  recovery.observe(
    { id: "second", results: [{ isError: false, signature: "test:FAIL" }] },
    PROFILE,
  );
  expect(recovery.snapshot()).toStrictEqual({ steps: 0, successes: 0, lastTurn: "second" });
  recovery.observe({ id: "third", results: [{ isError: false, signature: "test:PASS" }] }, PROFILE);
  expect(recovery.snapshot()).toStrictEqual({ steps: 0, successes: 1, lastTurn: "third" });
});
it("forgets a signature after the window of recent calls", () => {
  const recovery = new RecoveryEffort();
  recovery.observe({ id: "first", results: [{ isError: false, signature: "old" }] }, PROFILE);
  recovery.observe(
    {
      id: "many",
      results: [
        { isError: false, signature: "1" },
        { isError: false, signature: "2" },
        { isError: false, signature: "3" },
        { isError: false, signature: "4" },
        { isError: false, signature: "5" },
        { isError: false, signature: "6" },
        { isError: false, signature: "7" },
        { isError: false, signature: "8" },
      ],
    },
    PROFILE,
  );
  recovery.observe(
    {
      id: "late",
      results: [{ isError: false, signature: "old" }, { isError: false }],
    },
    PROFILE,
  );
  expect(recovery.snapshot()).toStrictEqual({ steps: 0, successes: 1, lastTurn: "late" });
});
it("refills the repeat window from a resumed branch without recounting the floor", () => {
  const recovery = new RecoveryEffort();
  const call = {
    message: {
      content: [{ arguments: { command: "bun test" }, id: "a", name: "bash", type: "toolCall" }],
      role: "assistant",
    },
    type: "message",
  };
  const failed = {
    message: {
      content: [{ text: "FAIL", type: "text" }],
      isError: false,
      role: "toolResult",
      timestamp: 0,
      toolCallId: "a",
      toolName: "bash",
    },
    type: "message",
  };
  recovery.restore({ steps: 0, successes: 0, lastTurn: "before" }, [call, failed]);
  expect(recovery.snapshot()).toStrictEqual({ steps: 0, successes: 0, lastTurn: "before" });
  recovery.observe(
    {
      id: "after",
      results: [
        {
          isError: false,
          signature:
            'bash\u000022\u0000{"command":"bun test"}\u0000{"command":"bun test"}\u0000false\u00004\u0000FAIL\u0000FAIL',
        },
      ],
    },
    PROFILE,
  );
  expect(recovery.snapshot()).toStrictEqual({ steps: 0, successes: 0, lastTurn: "after" });
});
it("stops failure pressure at what the work levels can express, so recovery stays quick", () => {
  const recovery = new RecoveryEffort();
  recovery.observe({ id: "f1", results: [{ isError: true }] }, PROFILE);
  recovery.observe({ id: "f2", results: [{ isError: true }] }, PROFILE);
  expect(recovery.observe({ id: "f3", results: [{ isError: true }] }, PROFILE)).toBe(false);
  expect(recovery.snapshot()).toStrictEqual({ steps: 2, successes: 0, lastTurn: "f3" });
  recovery.observe({ id: "ok-1", results: [{ isError: false }] }, PROFILE);
  expect(recovery.observe({ id: "ok-2", results: [{ isError: false }] }, PROFILE)).toBe(true);
  expect(recovery.select(PROFILE, "medium")).toBe("high");
  expect(recovery.observe({ id: "f4", results: [{ isError: true }] }, undefined)).toBe(false);
  expect(recovery.snapshot()).toStrictEqual({ steps: 1, successes: 0, lastTurn: "ok-2" });
  expect(
    recovery.observe(
      { id: "narrow", results: [{ isError: true }] },
      { ...PROFILE, operational: ["medium"] },
    ),
  ).toBe(true);
  expect(recovery.snapshot()).toStrictEqual({ steps: 0, successes: 0, lastTurn: "narrow" });
});
it("holds the floor on a malformed call instead of raising it", () => {
  const recovery = new RecoveryEffort();
  recovery.observe({ id: "ok", results: [{ isError: false }] }, PROFILE);
  recovery.observe({ id: "bad-args", results: [{ isError: true, malformed: true }] }, PROFILE);
  expect(recovery.snapshot()).toStrictEqual({ steps: 0, successes: 0, lastTurn: "bad-args" });
});
it("lowers the floor at once when a failed input later succeeds", () => {
  const recovery = new RecoveryEffort();
  recovery.observe({ id: "f1", results: [{ isError: true, inputKey: "bash test" }] }, PROFILE);
  recovery.observe({ id: "f2", results: [{ isError: true, inputKey: "bash test" }] }, PROFILE);
  expect(
    recovery.observe({ id: "fix", results: [{ isError: false, inputKey: "bash test" }] }, PROFILE),
  ).toBe(true);
  expect(recovery.snapshot()).toStrictEqual({ steps: 1, successes: 0, lastTurn: "fix" });
});
it("claims one change-approach note for a run of failures at the cap until a round succeeds", () => {
  const recovery = new RecoveryEffort();
  const failure = { isError: true, inputKey: "bash build" };
  recovery.observe({ id: "f1", results: [failure] }, PROFILE);
  recovery.observe({ id: "f2", results: [failure] }, PROFILE);
  recovery.observe({ id: "f3", results: [failure] }, PROFILE);
  expect(recovery.claimStuckNote(failure, PROFILE)).toBe(false);
  recovery.observe({ id: "f4", results: [failure] }, PROFILE);
  expect(recovery.claimStuckNote({ isError: true, malformed: true }, PROFILE)).toBe(false);
  expect(recovery.claimStuckNote(failure, undefined)).toBe(false);
  expect(recovery.claimStuckNote(failure, PROFILE)).toBe(true);
  expect(recovery.claimStuckNote(failure, PROFILE)).toBe(false);
  recovery.observe({ id: "ok", results: [{ isError: false }] }, PROFILE);
  recovery.observe({ id: "f5", results: [failure] }, PROFILE);
  expect(recovery.claimStuckNote(failure, PROFILE)).toBe(false);
});
