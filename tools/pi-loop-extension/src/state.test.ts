// This TypeScript file is executed with Bun.
import { expect, it, vi } from "vitest";
import {
  createLoopState,
  latestLoopState,
  LOOP_STATE_ENTRY_TYPE,
  persistLoopState,
  restoredPendingContinuations,
} from "./state.ts";

const EMPTY_STATE = {
  jobs: [],
  nextId: 1,
  paused: false,
  pendingContinuations: [],
  runningContinuation: undefined,
} as const;

it("persists and restores the latest valid session state", () => {
  const writer = vi.fn();
  const state = createLoopState({
    ...EMPTY_STATE,
    jobs: [
      {
        id: 1,
        intervalMs: 60_000,
        nextRunAt: 120_000,
        prompt: "inspect",
        reason: "wait",
        remainingMs: 30_000,
        submittedAt: 60_000,
      },
    ],
  });
  persistLoopState(writer, { ...state, runningContinuation: undefined });

  expect(writer).toHaveBeenCalledWith(LOOP_STATE_ENTRY_TYPE, state);
  expect(
    latestLoopState([
      { customType: LOOP_STATE_ENTRY_TYPE, data: {}, type: "custom" },
      { customType: LOOP_STATE_ENTRY_TYPE, data: state, type: "custom" },
    ]),
  ).toEqual(state);
  const continuation = createLoopState({ ...EMPTY_STATE, runningContinuation: "continue task" });
  expect(restoredPendingContinuations(continuation)).toEqual(["continue task"]);
  expect(
    restoredPendingContinuations({
      ...continuation,
      pendingContinuations: ["continue task"],
    }),
  ).toEqual(["continue task"]);
});

it("ignores unrelated and malformed session entries", () => {
  expect(
    latestLoopState([
      null,
      { type: "message" },
      { customType: "other", data: EMPTY_STATE, type: "custom" },
      { customType: LOOP_STATE_ENTRY_TYPE, data: null, type: "custom" },
      { customType: LOOP_STATE_ENTRY_TYPE, data: { ...EMPTY_STATE, jobs: [null] }, type: "custom" },
      { customType: LOOP_STATE_ENTRY_TYPE, data: { ...EMPTY_STATE, jobs: [{}] }, type: "custom" },
      { customType: LOOP_STATE_ENTRY_TYPE, data: { ...EMPTY_STATE, paused: "no" }, type: "custom" },
      {
        customType: LOOP_STATE_ENTRY_TYPE,
        data: { ...EMPTY_STATE, pendingContinuations: [1] },
        type: "custom",
      },
      {
        customType: LOOP_STATE_ENTRY_TYPE,
        data: { ...EMPTY_STATE, runningContinuation: 1 },
        type: "custom",
      },
    ]),
  ).toBeUndefined();
  expect(restoredPendingContinuations(createLoopState(EMPTY_STATE))).toEqual([]);
  persistLoopState(undefined, EMPTY_STATE);
});
