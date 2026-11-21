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
export interface TaskPatch {
  readonly note?: string;
  readonly status?: Status;
  readonly title?: string;
  readonly weight?: number;
}

export const EMPTY_TIMELINE: Timeline = { tasks: [] };

/** How much of a leaf's own weight counts as finished. A task with subtasks takes their score. */
const SHARE: Readonly<Record<Status, number>> = { active: 0.5, blocked: 0, done: 1, pending: 0 };

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

/** Weighted share of `tasks` that is finished: subtasks replace a task's own status. */
function share(timeline: Timeline, tasks: readonly Task[]): number {
  let weight = 0;
  let done = 0;
  for (const task of tasks) {
    const subtasks = children(timeline, task.id);
    const value = subtasks.length === 0 ? SHARE[task.status] : share(timeline, subtasks);
    weight += task.weight;
    done += task.weight * value;
  }
  return weight === 0 ? 0 : done / weight;
}

/** Weighted share of the work under `parent` that is finished, from 0 to 1. */
export function progress(timeline: Timeline, parent: string | null = null): number {
  return share(timeline, children(timeline, parent));
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
/** The Harness keeps unfinished work below 100 at its two decimals. */
const INCOMPLETE_LIMIT = 99.99;

/** The Harness's formatProgress: two decimals, and work that is not finished never reads 100. */
function formatProgress(value: number): string {
  const percent = Math.max(0, Math.min(1, value)) * 100;
  return Math.min(percent, value < 1 ? INCOMPLETE_LIMIT : 100).toFixed(2);
}

function bar(value: number, cells: number): string {
  const filled = Math.round(Math.max(0, Math.min(1, value)) * cells);
  return `${"█".repeat(filled)}${"░".repeat(cells - filled)}`;
}

function taskLine(timeline: Timeline, task: Task): string {
  // The bar opens every line, so the bars line up in a column and the indented glyph reads the depth.
  const value = share(timeline, [task]);
  const percent = `${formatProgress(value)}%`;
  const note = task.note === "" ? "" : ` — ${task.note}`;
  return `${bar(value, TASK_CELLS)} ${"  ".repeat(depth(timeline, task))}${GLYPH[task.status]} ${task.id} ${task.title} ${percent}${note}`;
}

function lines(timeline: Timeline, parent: string | null, out: string[]): string[] {
  for (const task of children(timeline, parent)) {
    out.push(taskLine(timeline, task));
    lines(timeline, task.id, out);
  }
  return out;
}

/** The tree as lines: progress first, then each task indented under its parent, with its id. */
export function renderTimeline(timeline: Timeline): string[] {
  if (timeline.tasks.length === 0) {
    return ["No tasks."];
  }
  const value = progress(timeline);
  const head = `${bar(value, HEAD_CELLS)} ${formatProgress(value)}% · ${String(timeline.tasks.length)} tasks`;
  return [head, ...lines(timeline, null, [])];
}

export function parseTimeline(text: string): Timeline {
  const data: unknown = JSON.parse(text);
  if (typeof data !== "object" || data === null || !Array.isArray((data as Timeline).tasks)) {
    throw new Error("Invalid timeline state.");
  }
  return data as Timeline;
}
