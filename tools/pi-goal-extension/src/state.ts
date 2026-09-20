// Runs with Bun.
import {
  array,
  integer,
  literal,
  maxValue,
  minLength,
  minValue,
  nullable,
  number,
  object,
  parse,
  picklist,
  pipe,
  safeParse,
  string,
  trim,
} from "valibot";

export interface GoalBlocker {
  readonly key: string;
  readonly count: number;
  readonly turn: number;
}
export interface GoalWait {
  readonly until: number;
  readonly reason: string;
}
export interface GoalState {
  readonly version: 1;
  readonly id: string;
  readonly sessionId: string;
  readonly revision: number;
  readonly objective: string;
  readonly status:
    | "active"
    | "paused"
    | "blocked"
    | "budget_limited"
    | "complete";
  readonly tokenBudget: number | null;
  readonly tokensUsed: number;
  readonly elapsedMs: number;
  readonly createdAt: number;
  readonly updatedAt: number;
  readonly turn: number;
  readonly noProgressTurns: number;
  readonly blocker: GoalBlocker | null;
  readonly wait: GoalWait | null;
  readonly tasks: readonly string[];
  readonly reason: string | null;
}
export interface CreateGoalInput {
  readonly id: string;
  readonly sessionId: string;
  readonly objective: string;
  readonly tokenBudget: number | null;
  readonly now: number;
}
export interface GoalUpdateInput {
  readonly goal: GoalState;
  readonly status: "complete" | "blocked";
  readonly reason: string;
  readonly now: number;
}
export interface UsageInput {
  readonly goal: GoalState;
  readonly tokens: number;
  readonly elapsedMs: number;
  readonly now: number;
}
export interface RestoreInput {
  readonly entries: readonly unknown[];
  readonly sessionId: string;
  readonly now: number;
}

export const GOAL_ENTRY: string = "pi-goal-state-v1";
export const BLOCKER_TURNS: number = 3;
const INTEGER = pipe(
  number(),
  integer(),
  minValue(0),
  maxValue(Number.MAX_SAFE_INTEGER),
);
const TEXT = pipe(string(), trim(), minLength(1));
const POSITIVE_INTEGER = pipe(INTEGER, minValue(1));
const GOAL_SCHEMA = object({
  version: literal(1),
  id: TEXT,
  sessionId: TEXT,
  revision: INTEGER,
  objective: TEXT,
  status: picklist([
    "active",
    "paused",
    "blocked",
    "budget_limited",
    "complete",
  ]),
  tokenBudget: nullable(POSITIVE_INTEGER),
  tokensUsed: INTEGER,
  elapsedMs: INTEGER,
  createdAt: INTEGER,
  updatedAt: INTEGER,
  turn: INTEGER,
  noProgressTurns: INTEGER,
  blocker: nullable(
    object({ key: TEXT, count: POSITIVE_INTEGER, turn: INTEGER }),
  ),
  wait: nullable(object({ until: INTEGER, reason: TEXT })),
  tasks: array(TEXT),
  reason: nullable(TEXT),
});
const ENTRY_SCHEMA = object({
  type: literal("custom"),
  customType: literal(GOAL_ENTRY),
  data: nullable(GOAL_SCHEMA),
});

export function createGoal(input: CreateGoalInput): GoalState {
  return parse(GOAL_SCHEMA, {
    version: 1,
    id: input.id,
    sessionId: input.sessionId,
    revision: 0,
    objective: input.objective,
    status: "active",
    tokenBudget: input.tokenBudget,
    tokensUsed: 0,
    elapsedMs: 0,
    createdAt: input.now,
    updatedAt: input.now,
    turn: 0,
    noProgressTurns: 0,
    blocker: null,
    wait: null,
    tasks: [],
    reason: null,
  });
}

export function restoreGoal(input: RestoreInput): GoalState | null {
  const entry: unknown = [...input.entries]
    .reverse()
    .find(
      (value) =>
        typeof value === "object" &&
        value !== null &&
        "type" in value &&
        value.type === "custom" &&
        "customType" in value &&
        value.customType === GOAL_ENTRY,
    );
  const parsed = safeParse(ENTRY_SCHEMA, entry);
  if (!parsed.success || parsed.output.data === null) return null;
  const goal: GoalState = parsed.output.data;
  return goal.sessionId === input.sessionId
    ? goal
    : {
        ...goal,
        sessionId: input.sessionId,
        revision: goal.revision + 1,
        status: "paused",
        wait: null,
        tasks: [],
        updatedAt: input.now,
        reason:
          "Inherited goal is paused; explicitly resume it in this session.",
      };
}

export function pauseGoal(goal: GoalState, now: number): GoalState {
  return {
    ...goal,
    status: "paused",
    revision: goal.revision + 1,
    wait: null,
    updatedAt: now,
  };
}

export function resumeGoal(goal: GoalState, now: number): GoalState {
  if (goal.status === "complete")
    throw new Error("Create a new goal instead of resuming a completed goal.");
  if (goal.tokenBudget !== null && goal.tokensUsed >= goal.tokenBudget) {
    throw new Error(
      "Goal token budget exhausted; explicitly change the budget before resuming.",
    );
  }
  return {
    ...goal,
    status: "active",
    revision: goal.revision + 1,
    blocker: null,
    noProgressTurns: 0,
    wait: null,
    reason: null,
    updatedAt: now,
  };
}

function nextBlocker(goal: GoalState, reason: string): GoalBlocker {
  const previous: GoalBlocker | null = goal.blocker;
  if (previous?.key !== reason)
    return { key: reason, count: 1, turn: goal.turn };
  if (previous.turn === goal.turn) return previous;
  return {
    key: reason,
    count: previous.turn === goal.turn - 1 ? previous.count + 1 : 1,
    turn: goal.turn,
  };
}

export function updateGoal(input: GoalUpdateInput): GoalState {
  const reason: string = parse(TEXT, input.reason);
  if (input.goal.status !== "active" && input.status !== "complete")
    throw new Error("Only an active goal can be updated by the agent.");
  const blocker: GoalBlocker = nextBlocker(input.goal, reason);
  const blocked: boolean =
    input.status === "blocked" && blocker.count >= BLOCKER_TURNS;
  const status: GoalState["status"] = blocked ? "blocked" : input.goal.status;
  return {
    ...input.goal,
    status: input.status === "complete" ? "complete" : status,
    revision: input.goal.revision + 1,
    blocker: input.status === "blocked" ? blocker : null,
    reason,
    wait: null,
    updatedAt: input.now,
  };
}

export function accountUsage(input: UsageInput): GoalState {
  const tokensUsed: number = parse(
    INTEGER,
    input.goal.tokensUsed + parse(INTEGER, input.tokens),
  );
  const elapsedMs: number = parse(
    INTEGER,
    input.goal.elapsedMs + parse(INTEGER, input.elapsedMs),
  );
  const limited: boolean =
    input.goal.status === "active" &&
    input.goal.tokenBudget !== null &&
    tokensUsed >= input.goal.tokenBudget;
  return {
    ...input.goal,
    tokensUsed,
    elapsedMs,
    status: limited ? "budget_limited" : input.goal.status,
    revision: limited ? input.goal.revision + 1 : input.goal.revision,
    reason: limited
      ? "Explicit goal token budget exhausted."
      : input.goal.reason,
    updatedAt: input.now,
  };
}
