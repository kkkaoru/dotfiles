// This TypeScript file is executed with Bun.
export const STATUSES = ["pending", "active", "paused", "blocked", "done", "retired"] as const;
export type Status = (typeof STATUSES)[number];
export const PRIORITIES = ["P0", "P1", "P2", "P3"] as const;
export type Priority = (typeof PRIORITIES)[number];
export const MODES = ["serial", "parallel"] as const;
export type Mode = (typeof MODES)[number];

export interface Task {
  /** Units finished and units in total: 3 of 5 is a task that is more than half done. */
  readonly done: readonly [number, number];
  readonly id: string;
  readonly mode: Mode;
  readonly note: string;
  readonly parent: string | null;
  readonly priority: Priority | null;
  /** Minutes still expected, or null when nobody knows. */
  readonly remaining: number | null;
  readonly status: Status;
  readonly title: string;
  readonly weight: number;
}
/** One goal's plan: a title and the tasks under it. */
export interface Timeline {
  readonly id: string;
  readonly tasks: readonly Task[];
  readonly title: string;
}
/** Every plan of a session, and which one the widget shows. */
export interface Board {
  readonly active: string;
  readonly trees: readonly Timeline[];
}

export interface TaskPatch {
  readonly done?: readonly [number, number];
  readonly mode?: Mode;
  readonly note?: string;
  readonly priority?: Priority | null;
  readonly remaining?: number | null;
  readonly status?: Status;
  readonly title?: string;
  readonly weight?: number;
}

const BLANK: Timeline = { id: "tree-1", tasks: [], title: "" };

export const EMPTY_BOARD: Board = { active: BLANK.id, trees: [BLANK] };

/** The reference keeps unfinished work just below 1, so a tree that is still running never reads 100. */
const CAP = 0.9999;

/** The four categories the segmented bar shows; paused and pending share the quiet one. */
export interface Parts {
  readonly active: number;
  readonly blocked: number;
  readonly done: number;
  readonly pending: number;
}
export interface Measure {
  readonly parts: Parts;
  readonly progress: number;
  readonly remaining: number | null;
  readonly status: Status;
}

/** The state the unfinished part of a task is in, as the bar shows it. */
function remainderState(status: Status): "active" | "blocked" | "pending" {
  return status === "active" || status === "blocked" ? status : "pending";
}

function taskOf(timeline: Timeline, id: string): Task {
  const found = timeline.tasks.find((candidate) => candidate.id === id);
  if (found === undefined) {
    throw new Error(`Unknown task ${id}.`);
  }
  return found;
}

/** A retired task is out of the plan, and so is everything under it. */
function children(timeline: Timeline, parent: string | null): readonly Task[] {
  return timeline.tasks.filter((child) => child.parent === parent && child.status !== "retired");
}

function leaf(task: Task): Measure {
  const share = task.status === "done" ? 1 : task.done[0] / task.done[1];
  const rest = remainderState(task.status);
  return {
    parts: { active: 0, blocked: 0, done: share, pending: 0, [rest]: 1 - share },
    progress: share,
    remaining: task.status === "done" ? 0 : task.remaining,
    status: task.status,
  };
}

/** The state of a group: done only when every child is, and otherwise the loudest one wins. */
function groupStatus(states: readonly Status[]): Status {
  if (states.every((state) => state === "done")) {
    return "done";
  }
  for (const status of ["active", "blocked", "paused"] as const) {
    if (states.includes(status)) {
      return status;
    }
  }
  return "pending";
}

/** The bar's four shares: finished work plus the rest, normalised across the children's states. */
function splitParts(
  values: readonly { readonly task: Task; readonly value: Measure }[],
  largest: number,
  total: number,
  share: number,
  status: Status,
): Parts {
  const split: Record<keyof Parts, number> = { active: 0, blocked: 0, done: share, pending: 0 };
  for (const { task: child, value } of values) {
    for (const key of ["active", "blocked", "pending"] as const) {
      split[key] += (child.weight / largest / total) * value.parts[key];
    }
  }
  const rest = split.active + split.pending + split.blocked;
  for (const key of ["active", "pending", "blocked"] as const) {
    split[key] = rest === 0 ? 0 : (split[key] / rest) * (1 - share);
  }
  if (rest === 0) {
    split[remainderState(status)] = 1 - share;
  }
  return split;
}

/** One task without an estimate makes the group unknown rather than wrong. */
function sumEstimate(
  known: readonly number[],
  durations: readonly (number | null)[],
  parallel: boolean,
): number | null {
  if (known.length !== durations.length) {
    return null;
  }
  return parallel ? Math.max(...known) : known.reduce((sum, value) => sum + value, 0);
}

/** The reference's measureAll: subtasks replace a task's own status, weighted and normalised. */
function measure(timeline: Timeline, parent: string | null): Measure | undefined {
  const rows = children(timeline, parent);
  if (rows.length === 0) {
    return undefined;
  }
  const values = rows.map((child) => ({
    task: child,
    value: measure(timeline, child.id) ?? leaf(child),
  }));
  const largest = Math.max(1, ...rows.map((child) => child.weight));
  const total = Math.max(
    1,
    values.reduce((sum, { task: child }) => sum + child.weight / largest, 0),
  );
  const status = groupStatus(values.map(({ value }) => value.status));
  const share =
    status === "done"
      ? 1
      : Math.min(
          CAP,
          values.reduce(
            (sum, { task: child, value }) => sum + (child.weight / largest) * value.progress,
            0,
          ) / total,
        );
  const durations = values.map(({ value }) => value.remaining);
  const known = durations.filter((value): value is number => value !== null);
  const parallel = timeline.tasks.some(
    (candidate) => candidate.id === parent && candidate.mode === "parallel",
  );
  return {
    parts: splitParts(values, largest, total, share, status),
    progress: share,
    remaining: sumEstimate(known, durations, parallel),
    status,
  };
}

function measured(timeline: Timeline, taskId: string | null): Measure {
  const own = measure(timeline, taskId);
  if (own !== undefined) {
    return own;
  }
  return taskId === null
    ? {
        parts: { active: 0, blocked: 0, done: 0, pending: 1 },
        progress: 0,
        remaining: 0,
        status: "pending",
      }
    : leaf(taskOf(timeline, taskId));
}

/** How the work under `parent` splits by state, so the tree's bar can be drawn in segments. */
export function parts(timeline: Timeline, parent: string | null = null): Parts {
  return measured(timeline, parent).parts;
}

/** Weighted share of the work under `parent` that is finished, from 0 to 1. */
export function progress(timeline: Timeline, parent: string | null = null): number {
  return measured(timeline, parent).progress;
}

/** One task's own measure, whether it is a leaf or a group. */
export function measureOf(timeline: Timeline, id: string): Measure {
  taskOf(timeline, id);
  return measured(timeline, id);
}

/** Minutes left for the whole tree, or null when a task has no estimate. */
export function estimate(timeline: Timeline): number | null {
  return measured(timeline, null).remaining;
}

export function addTask(
  timeline: Timeline,
  input: { readonly parent?: string | null; readonly title: string; readonly weight?: number },
): { readonly id: string; readonly timeline: Timeline } {
  const parent = input.parent ?? null;
  if (parent !== null) {
    taskOf(timeline, parent);
  }
  const id = `t${String(timeline.tasks.length + 1)}`;
  const added: Task = {
    done: [0, 1],
    id,
    mode: "serial",
    note: "",
    parent,
    priority: null,
    remaining: null,
    status: "pending",
    title: input.title,
    weight: input.weight ?? 1,
  };
  return { id, timeline: { ...timeline, tasks: [...timeline.tasks, added] } };
}

export function updateTask(timeline: Timeline, id: string, patch: TaskPatch): Timeline {
  taskOf(timeline, id);
  return {
    ...timeline,
    tasks: timeline.tasks.map((entry) => (entry.id === id ? { ...entry, ...patch } : entry)),
  };
}

/** The tree the widget shows, which is the first one when the recorded id is gone. */
export function activeTree(board: Board): Timeline {
  return board.trees.find((tree) => tree.id === board.active) ?? board.trees[0] ?? BLANK;
}

/** Show an existing plan. */
export function selectTree(board: Board, id: string): Board {
  if (!board.trees.some((tree) => tree.id === id)) {
    throw new Error(`Unknown tree ${id}.`);
  }
  return { ...board, active: id };
}

/**
 * Start a plan for a goal, and make it the one the widget shows. A goal that already has a plan gets
 * that one back, so resuming work never forks the tree.
 */
export function startTree(
  board: Board,
  title: string,
): { readonly board: Board; readonly id: string } {
  // A session that has not named a plan yet gets this one, instead of an empty plan beside it.
  const blank = board.trees.length === 1 ? board.trees[0] : undefined;
  if (blank?.tasks.length === 0 && blank.title === "") {
    return { board: { active: blank.id, trees: [{ ...blank, title }] }, id: blank.id };
  }
  const known = board.trees.find((tree) => tree.title === title && title !== "");
  if (known !== undefined) {
    return { board: selectTree(board, known.id), id: known.id };
  }
  const id = `tree-${String(board.trees.length + 1)}`;
  return { board: { active: id, trees: [...board.trees, { id, tasks: [], title }] }, id };
}

/** Replace the active tree, keeping the board's order and the other plans. */
export function withTree(board: Board, timeline: Timeline): Board {
  return {
    ...board,
    trees: board.trees.map((tree) => (tree.id === timeline.id ? timeline : tree)),
  };
}

function isStatus(value: unknown): value is Status {
  return typeof value === "string" && (STATUSES as readonly string[]).includes(value);
}

function isPriority(value: unknown): value is Priority {
  return typeof value === "string" && (PRIORITIES as readonly string[]).includes(value);
}

function invalid(): never {
  throw new TypeError("Invalid timeline state.");
}

function readText(value: unknown): string {
  return typeof value === "string" ? value : invalid();
}

function readStatus(value: unknown): Status {
  return isStatus(value) ? value : invalid();
}

function readParent(value: unknown): string | null {
  return value === null || typeof value === "string" ? value : invalid();
}

function readWeight(value: unknown): number {
  return typeof value === "number" && value > 0 ? value : invalid();
}

/** A task recorded before done units was either finished or not. */
function readDone(value: unknown, status: Status): readonly [number, number] {
  return Array.isArray(value) && typeof value[0] === "number" && typeof value[1] === "number"
    ? [value[0], value[1]]
    : [status === "done" ? 1 : 0, 1];
}

/** Read one stored task, filling what an older sidecar did not carry. */
function readTask(value: unknown): Task {
  if (typeof value !== "object" || value === null) {
    return invalid();
  }
  const stored = value as Record<string, unknown>;
  const status = readStatus(stored["status"]);
  return {
    done: readDone(stored["done"], status),
    id: readText(stored["id"]),
    mode: stored["mode"] === "parallel" ? "parallel" : "serial",
    note: typeof stored["note"] === "string" ? stored["note"] : "",
    parent: readParent(stored["parent"]),
    priority: isPriority(stored["priority"]) ? stored["priority"] : null,
    remaining:
      typeof stored["remaining"] === "number" && stored["remaining"] >= 0
        ? stored["remaining"]
        : null,
    status,
    title: readText(stored["title"]),
    weight: readWeight(stored["weight"]),
  };
}

function readTasks(value: unknown): readonly Task[] {
  return Array.isArray(value) ? value.map((entry) => readTask(entry)) : invalid();
}

function readTree(value: unknown): Timeline {
  if (typeof value !== "object" || value === null) {
    return invalid();
  }
  const { id, tasks, title } = value as Record<string, unknown>;
  return {
    id: readText(id),
    tasks: readTasks(tasks),
    title: typeof title === "string" ? title : "",
  };
}

/** Read a stored board, and lift a sidecar that predates trees into one. */
export function parseBoard(text: string): Board {
  const data: unknown = JSON.parse(text);
  if (typeof data !== "object" || data === null) {
    return invalid();
  }
  const { active, tasks, trees } = data as Record<string, unknown>;
  // A sidecar written before plans existed is one plan.
  if (Array.isArray(tasks)) {
    return { active: "tree-1", trees: [{ id: "tree-1", tasks: readTasks(tasks), title: "" }] };
  }
  if (!Array.isArray(trees) || trees.length === 0) {
    return invalid();
  }
  const read = trees.map((entry) => readTree(entry));
  return {
    active:
      typeof active === "string" && read.some((tree) => tree.id === active)
        ? active
        : (read[0] ?? BLANK).id,
    trees: read,
  };
}
