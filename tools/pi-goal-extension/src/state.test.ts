// Runs with Bun; Vitest uses in-memory session entries.
import { expect, it } from "vitest";
import {
  accountUsage,
  createGoal,
  type GoalState,
  pauseGoal,
  restoreGoal,
  resumeGoal,
  updateGoal,
} from "./state.ts";

const GOAL: GoalState = createGoal({
  id: "goal-1",
  sessionId: "session-1",
  objective: "Verify the change",
  tokenBudget: null,
  now: 100,
});

it("creates a full objective with no invented token budget", () => {
  expect(GOAL).toStrictEqual({
    version: 1,
    id: "goal-1",
    sessionId: "session-1",
    revision: 0,
    objective: "Verify the change",
    status: "active",
    tokenBudget: null,
    tokensUsed: 0,
    elapsedMs: 0,
    createdAt: 100,
    updatedAt: 100,
    turn: 0,
    noProgressTurns: 0,
    blocker: null,
    wait: null,
    tasks: [],
    reason: null,
  });
});
it.each([
  0,
  -1,
  1.5,
  Number.NaN,
  Number.POSITIVE_INFINITY,
  Number.MAX_SAFE_INTEGER + 1,
])("rejects invalid budget %s", (tokenBudget) => {
  expect(() =>
    createGoal({
      id: "g",
      sessionId: "s",
      objective: "work",
      tokenBudget,
      now: 1,
    }),
  ).toThrow();
});
it("rejects empty objectives", () => {
  expect(() =>
    createGoal({
      id: "g",
      sessionId: "s",
      objective: "  ",
      tokenBudget: null,
      now: 1,
    }),
  ).toThrow();
});
it("restores latest goal and ignores unrelated entries", () => {
  expect(
    restoreGoal({
      entries: [
        null,
        { type: "custom" },
        { type: "message", customType: "pi-goal-state-v1" },
        { type: "custom", customType: "other" },
        { type: "custom", customType: "pi-goal-state-v1", data: GOAL },
      ],
      sessionId: "session-1",
      now: 200,
    })?.id,
  ).toBe("goal-1");
});
it("empty, cleared and malformed states fail closed", () => {
  expect(
    restoreGoal({ entries: [], sessionId: "session-1", now: 200 }),
  ).toBeNull();
  expect(
    restoreGoal({
      entries: [
        { type: "custom", customType: "pi-goal-state-v1", data: GOAL },
        { type: "custom", customType: "pi-goal-state-v1", data: null },
      ],
      sessionId: "session-1",
      now: 200,
    }),
  ).toBeNull();
  expect(
    restoreGoal({
      entries: [
        { type: "custom", customType: "pi-goal-state-v1", data: GOAL },
        { type: "custom", customType: "pi-goal-state-v1", data: {} },
      ],
      sessionId: "session-1",
      now: 200,
    }),
  ).toBeNull();
});
it("fork restoration pauses and drops inherited process ownership", () => {
  const restored = restoreGoal({
    entries: [
      {
        type: "custom",
        customType: "pi-goal-state-v1",
        data: {
          ...GOAL,
          tasks: ["old-task"],
          wait: { until: 500, reason: "waiting" },
        },
      },
    ],
    sessionId: "fork",
    now: 200,
  });
  expect(restored?.status).toBe("paused");
  expect(restored?.sessionId).toBe("fork");
  expect(restored?.revision).toBe(1);
  expect(restored?.tasks).toStrictEqual([]);
  expect(restored?.wait).toBeNull();
});
it("pause invalidates tickets and resume clears blocker and waiting state", () => {
  const paused = pauseGoal(
    {
      ...GOAL,
      blocker: { key: "x", count: 3, turn: 0 },
      wait: { until: 300, reason: "wait" },
    },
    200,
  );
  expect(paused.status).toBe("paused");
  expect(paused.revision).toBe(1);
  expect(paused.wait).toBeNull();
  const resumed = resumeGoal(paused, 300);
  expect(resumed.status).toBe("active");
  expect(resumed.revision).toBe(2);
  expect(resumed.blocker).toBeNull();
  expect(resumed.reason).toBeNull();
});
it("resume rejects completed goals and exhausted budgets", () => {
  expect(() => resumeGoal({ ...GOAL, status: "complete" }, 200)).toThrow(
    "Create a new goal",
  );
  expect(() =>
    resumeGoal({ ...GOAL, tokenBudget: 10, tokensUsed: 10 }, 200),
  ).toThrow("budget exhausted");
  expect(
    resumeGoal({ ...GOAL, tokenBudget: 11, tokensUsed: 10 }, 200).status,
  ).toBe("active");
});
it("requires three consecutive turns for the same blocker, not duplicate calls", () => {
  const first = updateGoal({
    goal: GOAL,
    status: "blocked",
    reason: "Missing user consent",
    now: 200,
  });
  const duplicate = updateGoal({
    goal: first,
    status: "blocked",
    reason: "Missing user consent",
    now: 201,
  });
  const second = updateGoal({
    goal: { ...duplicate, turn: 1 },
    status: "blocked",
    reason: "Missing user consent",
    now: 300,
  });
  const third = updateGoal({
    goal: { ...second, turn: 2 },
    status: "blocked",
    reason: "Missing user consent",
    now: 400,
  });
  expect(first.status).toBe("active");
  expect(duplicate.blocker?.count).toBe(1);
  expect(second.blocker?.count).toBe(2);
  expect(third.status).toBe("blocked");
  expect(third.blocker?.count).toBe(3);
});
it("changed blockers and gaps reset consecutive counts", () => {
  const previous: GoalState = {
    ...GOAL,
    turn: 3,
    blocker: { key: "old", count: 2, turn: 1 },
  };
  expect(
    updateGoal({ goal: previous, status: "blocked", reason: "old", now: 200 })
      .blocker?.count,
  ).toBe(1);
  expect(
    updateGoal({ goal: previous, status: "blocked", reason: "new", now: 200 })
      .blocker?.count,
  ).toBe(1);
});
it("completion requires a nonempty audit and clears blocker", () => {
  expect(() =>
    updateGoal({ goal: GOAL, status: "complete", reason: " ", now: 200 }),
  ).toThrow();
  const completed = updateGoal({
    goal: GOAL,
    status: "complete",
    reason: "Tests passed; artifacts inspected",
    now: 200,
  });
  expect(completed.status).toBe("complete");
  expect(completed.blocker).toBeNull();
  expect(() =>
    updateGoal({ goal: completed, status: "blocked", reason: "x", now: 300 }),
  ).toThrow("Only an active goal");
});
it("counts tokens and elapsed time without an implicit budget", () => {
  const result = accountUsage({
    goal: GOAL,
    tokens: 20,
    elapsedMs: 30,
    now: 200,
  });
  expect(result.tokensUsed).toBe(20);
  expect(result.elapsedMs).toBe(30);
  expect(result.status).toBe("active");
});
it("reaching an explicit budget invalidates continuation tickets", () => {
  const result = accountUsage({
    goal: { ...GOAL, tokenBudget: 20 },
    tokens: 20,
    elapsedMs: 30,
    now: 200,
  });
  expect(result.status).toBe("budget_limited");
  expect(result.revision).toBe(1);
  expect(result.reason).toBe("Explicit goal token budget exhausted.");
  expect(
    accountUsage({
      goal: { ...GOAL, status: "complete", tokenBudget: 20 },
      tokens: 20,
      elapsedMs: 0,
      now: 200,
    }).status,
  ).toBe("complete");
  expect(
    accountUsage({
      goal: { ...GOAL, tokenBudget: 21 },
      tokens: 20,
      elapsedMs: 0,
      now: 200,
    }).status,
  ).toBe("active");
});
it("rejects invalid accounting values instead of corrupting persistence", () => {
  expect(() =>
    accountUsage({ goal: GOAL, tokens: -1, elapsedMs: 0, now: 200 }),
  ).toThrow();
  expect(() =>
    accountUsage({ goal: GOAL, tokens: 1, elapsedMs: Number.NaN, now: 200 }),
  ).toThrow();
});
