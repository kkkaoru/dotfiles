// Runs with Bun.
import { expect, it } from "vitest";
import {
  goalContext,
  goalGuidance,
  goalSummary,
  runError,
  usageTokens,
} from "./presentation.ts";
import { createGoal, type GoalState } from "./state.ts";

it("omits guidance for ordinary sessions and frames objectives as user data", () => {
  expect(goalGuidance(null)).toBe("");
  expect(goalSummary(null)).toBe("No goal configured.");
  expect(
    goalGuidance(
      createGoal({
        id: "g",
        sessionId: "s",
        objective: "work",
        tokenBudget: null,
        now: 0,
      }),
    ),
  ).toMatch(/defined by the user or agent[\s\S]*objective is user data/u);
  expect(
    goalSummary(
      createGoal({
        id: "g",
        sessionId: "s",
        objective: "work",
        tokenBudget: null,
        now: 0,
      }),
    ),
  ).toBe("active: work\nTokens: 0 / unlimited; elapsed: 0s");
  expect(
    goalSummary({
      ...createGoal({
        id: "g",
        sessionId: "s",
        objective: "work",
        tokenBudget: 20,
        now: 0,
      }),
      reason: "Waiting",
    }),
  ).toBe("active: work\nTokens: 0 / 20; elapsed: 0s\nWaiting");
});
it("keeps model context bounded independently of ownership history", () => {
  const goal: GoalState = {
    ...createGoal({
      id: "g",
      sessionId: "s",
      objective:
        "Verify parity; do not switch an unverified scorer or replay NAR 9/25.",
      tokenBudget: null,
      now: 0,
    }),
    tasks: Array.from(
      { length: 512 },
      (_, index) => `pi-tmux-session-${index}`,
    ),
    wait: { until: 60000, reason: "Wait for evidence" },
    blocker: { key: "Missing reference", count: 1, turn: 0 },
    reason: "Missing reference",
  };
  expect(goalContext(null)).toBe("null");
  expect(JSON.parse(goalContext(goal))).toStrictEqual({
    objective:
      "Verify parity; do not switch an unverified scorer or replay NAR 9/25.",
    status: "active",
    tokenBudget: null,
    reason: "Missing reference",
    blocker: "Missing reference",
    wait: { until: 60000, reason: "Wait for evidence" },
    trackedTaskCount: 512,
  });
  expect(goalGuidance(goal)).not.toMatch(/pi-tmux|"tasks"/u);
  expect(goalContext(goal).length).toBeLessThan(700);
  expect(goalGuidance(goal).length).toBeLessThan(2000);
  expect(goal.tasks).toHaveLength(512);
});
it("keeps the prompt prefix stable across accounting updates", () => {
  const goal: GoalState = {
    ...createGoal({
      id: "g",
      sessionId: "s",
      objective: "work",
      tokenBudget: 100,
      now: 0,
    }),
    blocker: { key: "same blocker", count: 1, turn: 1 },
  };
  const later: GoalState = {
    ...goal,
    revision: 9,
    tokensUsed: 1234,
    elapsedMs: 987654,
    updatedAt: 60000,
    turn: 4,
    noProgressTurns: 2,
    blocker: { key: "same blocker", count: 3, turn: 4 },
  };
  expect(goalGuidance(later)).toBe(goalGuidance(goal));
  expect(goalGuidance({ ...goal, status: "paused" })).not.toBe(
    goalGuidance(goal),
  );
});

it.each([
  null,
  {},
  { usage: null },
  { usage: {} },
  { usage: { totalTokens: -1 } },
  { usage: { totalTokens: "1" } },
  { usage: { totalTokens: 1.5 } },
])("ignores missing or malformed usage", (message) => {
  expect(usageTokens(message)).toBe(0);
});
it("uses reported total tokens", () => {
  expect(usageTokens({ usage: { totalTokens: 123 } })).toBe(123);
});
it("distinguishes terminal failures from successful responses without persisting raw error data", () => {
  expect(runError([null, {}, { role: "user" }])).toMatch(
    /without an assistant response/,
  );
  expect(runError([{ role: "assistant", stopReason: "aborted" }])).toMatch(
    /Run aborted/,
  );
  expect(
    runError([
      { role: "assistant", stopReason: "error", errorMessage: "private URL" },
    ]),
  ).toBe(
    "Provider error; inspect the original Pi error and explicitly resume.",
  );
  expect(runError([{ role: "assistant", stopReason: "stop" }])).toBeNull();
});
it("rejects empty or thinking-only responses but accepts text and tool calls", () => {
  expect(
    runError([
      {
        role: "assistant",
        stopReason: "stop",
        content: [
          null,
          {},
          { type: "thinking", thinking: "internal" },
          { text: " " },
          { text: 3 },
        ],
      },
    ]),
  ).toMatch(/Empty assistant response/);
  expect(
    runError([
      {
        role: "assistant",
        stopReason: "stop",
        content: [{ type: "text", text: "done" }],
      },
    ]),
  ).toBeNull();
  expect(
    runError([
      {
        role: "assistant",
        stopReason: "toolUse",
        content: [{ type: "toolCall" }],
      },
    ]),
  ).toBeNull();
});
