// This TypeScript file is executed with Bun.
import { StringEnum } from "@earendil-works/pi-ai";
import { Type, type Static } from "typebox";
import { renderTimeline } from "./render.ts";
import {
  activeTree,
  addTask,
  PRIORITIES,
  selectTree,
  startTree,
  STATUSES,
  updateTask,
  withTree,
  type Board,
  type TaskPatch,
  type Timeline,
} from "./tree.ts";

const statusLiterals = STATUSES.map((status) => Type.Literal(status));
const statusSchema = Type.Union(statusLiterals);
const modeSchema = Type.Union([Type.Literal("serial"), Type.Literal("parallel")]);
const priorityLiterals = PRIORITIES.map((priority) => Type.Literal(priority));
const prioritySchema = Type.Union(priorityLiterals);
const doneField = Type.Optional(
  Type.Tuple([Type.Number({ minimum: 0 }), Type.Number({ minimum: 0 })], {
    description: "Units finished and units in total, e.g. [1, 3] for a third done",
  }),
);
const modeField = Type.Optional(
  Type.Union([modeSchema], {
    description: "Whether the subtasks run one after another (default) or at the same time",
  }),
);
const priorityField = Type.Optional(
  Type.Union([prioritySchema, Type.Null()], { description: "P0 (first) to P3, or null" }),
);
const remainingField = Type.Optional(
  Type.Union([Type.Number({ minimum: 0 }), Type.Null()], {
    description: "Minutes still expected, or null when unknown",
  }),
);

export const parameters = Type.Object({
  action: StringEnum(["add", "update", "list", "start", "select"] as const),
  done: doneField,
  id: Type.Optional(Type.String({ description: "Task id, as shown by list" })),
  mode: modeField,
  note: Type.Optional(Type.String({ description: "Short note: the result or the blocker" })),
  parent: Type.Optional(Type.String({ description: "Parent task id, to make this a subtask" })),
  priority: priorityField,
  remaining: remainingField,
  status: Type.Optional(statusSchema),
  title: Type.Optional(Type.String({ description: "What the task is, or the new tree's goal" })),
  tree: Type.Optional(
    Type.String({ description: "Tree id: the plan to select, or the one a task belongs to" }),
  ),
  weight: Type.Optional(
    Type.Integer({ description: "Relative size among its siblings, default 1", minimum: 1 }),
  ),
});
export type Params = Static<typeof parameters>;

/** The fields a call may change, so a patch carries only the ones it was given. */
const FIELDS = [
  "done",
  "mode",
  "note",
  "priority",
  "remaining",
  "status",
  "title",
  "weight",
] as const;

function patchOf(params: Params): TaskPatch {
  const { done } = params;
  if (done !== undefined && (done[1] <= 0 || done[0] < 0 || done[0] > done[1])) {
    throw new Error("done needs units finished out of units in total, e.g. [1, 3].");
  }
  const patch: Record<string, unknown> = {};
  for (const field of FIELDS) {
    const value = params[field];
    if (value !== undefined) {
      patch[field] = value;
    }
  }
  return patch;
}

/** The tree a call works on: the active one, or the one it names. */
function target(board: Board, params: Params): Timeline {
  return params.tree === undefined ? activeTree(board) : activeTree(selectTree(board, params.tree));
}

function added(timeline: Timeline, params: Params): Timeline {
  if (params.title === undefined) {
    throw new Error("action add needs a title.");
  }
  return addTask(timeline, {
    parent: params.parent ?? null,
    title: params.title,
    ...(params.weight === undefined ? {} : { weight: params.weight }),
  }).timeline;
}

function updated(timeline: Timeline, params: Params): Timeline {
  if (params.id === undefined) {
    throw new Error("action update needs an id.");
  }
  return updateTask(timeline, params.id, patchOf(params));
}

export function apply(board: Board, params: Params): Board {
  switch (params.action) {
    case "add": {
      return withTree(board, added(target(board, params), params));
    }
    case "update": {
      return withTree(board, updated(target(board, params), params));
    }
    case "select": {
      if (params.tree === undefined) {
        throw new Error("action select needs a tree id.");
      }
      return selectTree(board, params.tree);
    }
    case "start": {
      if (params.title === undefined) {
        throw new Error("action start needs a title: the goal this plan is for.");
      }
      return startTree(board, params.title).board;
    }
    default: {
      return board;
    }
  }
}

/** A tree whose every task is done or retired has nothing left to track, so the editor gets its rows back. */
export function shows(timeline: Timeline): boolean {
  return timeline.tasks.some((task) => task.status !== "done" && task.status !== "retired");
}

/** The tree's lines, and the other plans when this session has more than one. */
export function boardText(board: Board): string {
  const lines = renderTimeline(activeTree(board));
  if (board.trees.length < 2) {
    return lines.join("\n");
  }
  const plans = board.trees.map((tree) =>
    [tree.id, tree.title, tree.id === board.active ? "(active)" : ""]
      .filter((part) => part !== "")
      .join(" "),
  );
  return `${lines.join("\n")}\nTrees: ${plans.join(", ")}`;
}
