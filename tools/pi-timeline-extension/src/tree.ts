// This TypeScript file is executed with Bun.
export const STATUSES = ["pending", "active", "done", "blocked"] as const;
export type Status = (typeof STATUSES)[number];

export interface Task {
  readonly id: string;
  readonly note: string;
  readonly parent: string | null;
  readonly status: Status;
  readonly title: string;
  readonly weight: number;
}
export interface Timeline {
  readonly tasks: readonly Task[];
}

/** What a piece of the display stands for, so a theme can give it a colour. */
export type Role = Status | "empty" | "muted" | "total";
/** Colour one piece; the default returns it unchanged, which is what tool output and tests use. */
export type Colour = (role: Role, text: string) => string;

const PLAIN: Colour = (_role, text) => text;
export interface TaskPatch {
  readonly note?: string;
  readonly status?: Status;
  readonly title?: string;
  readonly weight?: number;
}

export const EMPTY_TIMELINE: Timeline = { tasks: [] };

/** Younger than this, the tree's age is not worth the head's width. */
export const STALE_AFTER_MS = 5 * 60_000;

/** How long ago the tree changed, in the shortest form a head can carry. */
export function ageText(ms: number): string {
  const minutes = Math.max(1, Math.floor(ms / 60_000));
  if (minutes < 60) {
    return `${String(minutes)}m`;
  }
  const hours = Math.floor(minutes / 60);
  return hours < 24 ? `${String(hours)}h` : `${String(Math.floor(hours / 24))}d`;
}

/** How much of a leaf's own weight counts as finished. A task with subtasks takes their score. */
const SHARE: Readonly<Record<Status, number>> = { active: 0.5, blocked: 0, done: 1, pending: 0 };

/** How the work splits by the state of each part, which is what the reference draws as a segment. */
export interface Parts {
  readonly active: number;
  readonly blocked: number;
  readonly done: number;
  readonly pending: number;
}

const GLYPH: Readonly<Record<Status, string>> = {
  active: "▸",
  blocked: "✖",
  done: "✔",
  pending: "·",
};

function children(timeline: Timeline, parent: string | null): readonly Task[] {
  return timeline.tasks.filter((task) => task.parent === parent);
}

function find(timeline: Timeline, id: string): Task {
  const task = timeline.tasks.find((candidate) => candidate.id === id);
  if (task === undefined) {
    throw new Error(`Unknown task ${id}.`);
  }
  return task;
}

/** A leaf's work, split between what it finished and the state the rest is in. */
function leafParts(status: Status): Parts {
  const rest = status === "active" || status === "blocked" ? status : "pending";
  return { active: 0, blocked: 0, done: SHARE[status], pending: 0, [rest]: 1 - SHARE[status] };
}

/** Weighted split of `tasks`: subtasks replace a task's own status, as the reference measures them. */
function measure(timeline: Timeline, tasks: readonly Task[]): Parts {
  const total: Record<keyof Parts, number> = { active: 0, blocked: 0, done: 0, pending: 0 };
  let weight = 0;
  for (const task of tasks) {
    const subtasks = children(timeline, task.id);
    const value = subtasks.length === 0 ? leafParts(task.status) : measure(timeline, subtasks);
    weight += task.weight;
    for (const key of ["active", "blocked", "done", "pending"] as const) {
      total[key] += task.weight * value[key];
    }
  }
  if (weight === 0) {
    return { active: 0, blocked: 0, done: 0, pending: 1 };
  }
  return {
    active: total.active / weight,
    blocked: total.blocked / weight,
    done: total.done / weight,
    pending: total.pending / weight,
  };
}

/** How the work under `parent` splits by state, so the tree's bar can be drawn in segments. */
export function parts(timeline: Timeline, parent: string | null = null): Parts {
  return measure(timeline, children(timeline, parent));
}

/** Weighted share of the work under `parent` that is finished, from 0 to 1. */
export function progress(timeline: Timeline, parent: string | null = null): number {
  return parts(timeline, parent).done;
}

export function addTask(
  timeline: Timeline,
  input: { readonly parent?: string | null; readonly title: string; readonly weight?: number },
): { readonly id: string; readonly timeline: Timeline } {
  const parent = input.parent ?? null;
  if (parent !== null) {
    find(timeline, parent);
  }
  const id = `t${String(timeline.tasks.length + 1)}`;
  const task: Task = {
    id,
    note: "",
    parent,
    status: "pending",
    title: input.title,
    weight: input.weight ?? 1,
  };
  return { id, timeline: { tasks: [...timeline.tasks, task] } };
}

export function updateTask(timeline: Timeline, id: string, patch: TaskPatch): Timeline {
  find(timeline, id);
  return { tasks: timeline.tasks.map((task) => (task.id === id ? { ...task, ...patch } : task)) };
}

function parentOf(timeline: Timeline, id: string): string | null {
  for (const task of timeline.tasks) {
    if (task.id === id) {
      return task.parent;
    }
  }
  return null;
}

function depth(timeline: Timeline, task: Task): number {
  const { parent: first } = task;
  let level = 0;
  let parent = first;
  while (parent !== null) {
    level += 1;
    parent = parentOf(timeline, parent);
  }
  return level;
}

/** Cells in the tree's bar, and in a task's own: the Harness draws the tree's bar wider. */
const HEAD_CELLS = 10;
const TASK_CELLS = 6;
/** `100.00%` is the widest number, so every percentage ends in the same column. */
const PERCENT_WIDTH = 7;
/** The Harness keeps unfinished work below 100 at its two decimals. */
const INCOMPLETE_LIMIT = 99.99;

/** The Harness's formatProgress: two decimals, and work that is not finished never reads 100. */
function formatProgress(value: number): string {
  const percent = Math.max(0, Math.min(1, value)) * 100;
  return Math.min(percent, value < 1 ? INCOMPLETE_LIMIT : 100).toFixed(2);
}

function bar(value: number, cells: number, role: Role, colour: Colour): string {
  const filled = Math.round(Math.max(0, Math.min(1, value)) * cells);
  return `${colour(role, "█".repeat(filled))}${colour("empty", "░".repeat(cells - filled))}`;
}

/** The tree's bar in segments: finished, running, blocked and the rest, each in its own colour. */
function segments(value: Parts, cells: number, colour: Colour): string {
  let edge = 0;
  let drawn = 0;
  let text = "";
  // Only finished work fills the bar; running and blocked cells keep theirs in their own colour.
  for (const [role, glyph] of [
    ["done", "█"],
    ["active", "░"],
    ["blocked", "░"],
  ] as const) {
    edge += value[role];
    const upto = Math.round(edge * cells);
    if (upto > drawn) {
      text += colour(role, glyph.repeat(upto - drawn));
      drawn = upto;
    }
  }
  return drawn >= cells ? text : `${text}${colour("empty", "░".repeat(cells - drawn))}`;
}

function taskLine(timeline: Timeline, task: Task, colour: Colour): string {
  // The bar opens every line and the percentage follows it at a fixed width, so both line up down
  // The tree; the indent after them still reads the depth.
  const value = measure(timeline, [task]).done;
  const percent = colour(task.status, `${formatProgress(value)}%`.padStart(PERCENT_WIDTH));
  const note = task.note === "" ? "" : ` — ${task.note}`;
  return `${bar(value, TASK_CELLS, task.status, colour)} ${percent} ${"  ".repeat(depth(timeline, task))}${GLYPH[task.status]} ${task.id} ${task.title}${note}`;
}

function lines(timeline: Timeline, parent: string | null, out: string[], colour: Colour): string[] {
  for (const task of children(timeline, parent)) {
    out.push(taskLine(timeline, task, colour));
    lines(timeline, task.id, out, colour);
  }
  return out;
}

/** The tree as lines: progress first, then each task indented under its parent, with its id. */
export function renderTimeline(
  timeline: Timeline,
  colour: Colour = PLAIN,
  ageMs?: number,
): string[] {
  if (timeline.tasks.length === 0) {
    return ["No tasks."];
  }
  const split = parts(timeline);
  // A tree nobody has touched for a while is the one that lies, so its age rides in the head.
  const stale =
    ageMs !== undefined && ageMs >= STALE_AFTER_MS
      ? colour("muted", ` · updated ${ageText(ageMs)}`)
      : "";
  const head = `${segments(split, HEAD_CELLS, colour)} ${colour("muted", `${formatProgress(split.done)}%`)} · ${String(timeline.tasks.length)} tasks${stale}`;
  return [head, ...lines(timeline, null, [], colour)];
}

export function parseTimeline(text: string): Timeline {
  const data: unknown = JSON.parse(text);
  if (typeof data !== "object" || data === null || !Array.isArray((data as Timeline).tasks)) {
    throw new Error("Invalid timeline state.");
  }
  return data as Timeline;
}
