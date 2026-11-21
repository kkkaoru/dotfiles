// This TypeScript file is executed with Bun.
import {
  estimate,
  measureOf,
  parts,
  type Parts,
  type Status,
  type Task,
  type Timeline,
} from "./tree.ts";

/** What a piece of the display stands for, so a theme can give it a colour. */
export type Role = Status | "empty" | "muted" | "total";
/** Colour one piece; the default returns it unchanged, which is what tool output and tests use. */
export type Colour = (role: Role, text: string) => string;

const PLAIN: Colour = (_role, text) => text;

const GLYPH: Readonly<Record<Status, string>> = {
  active: "▸",
  blocked: "✖",
  done: "✔",
  paused: "⏸",
  pending: "·",
  retired: "–",
};

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

/** How long a task is expected to take, in the shortest form a row can carry. */
export function estimateText(minutes: number): string {
  const rounded = Math.max(1, Math.round(minutes));
  if (rounded < 60) {
    return `${String(rounded)}m`;
  }
  const hours = Math.floor(rounded / 60);
  const rest = rounded % 60;
  return rest === 0 ? `${String(hours)}h` : `${String(hours)}h${String(rest)}m`;
}

function depth(timeline: Timeline, own: Task): number {
  const parents = new Map(timeline.tasks.map((entry) => [entry.id, entry.parent]));
  let level = 0;
  let { parent } = own;
  while (parent !== null) {
    level += 1;
    parent = parents.get(parent) ?? null;
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

/** The tree's bar in segments: finished work fills it, and the rest takes the colour of its state. */
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

function taskLine(timeline: Timeline, own: Task, colour: Colour): string {
  const value = measureOf(timeline, own.id);
  const percent = colour(own.status, `${formatProgress(value.progress)}%`.padStart(PERCENT_WIDTH));
  const priority = own.priority === null ? "" : `[${own.priority}] `;
  const note = own.note === "" ? "" : ` — ${own.note}`;
  const left =
    value.remaining === null ||
    value.remaining === 0 ||
    own.status === "done" ||
    own.status === "retired"
      ? ""
      : ` · ~${estimateText(value.remaining)}`;
  // The bar opens every line and the percentage follows it at a fixed width, so both line up down
  // The tree; the indent after them still reads the depth.
  return `${bar(value.progress, TASK_CELLS, own.status, colour)} ${percent} ${"  ".repeat(depth(timeline, own))}${GLYPH[own.status]} ${priority}${own.id} ${own.title}${note}${left}`;
}

function lines(timeline: Timeline, parent: string | null, out: string[], colour: Colour): string[] {
  for (const child of timeline.tasks.filter((candidate) => candidate.parent === parent)) {
    out.push(taskLine(timeline, child, colour));
    lines(timeline, child.id, out, colour);
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
  const left = estimate(timeline);
  const named = timeline.title === "" ? "" : colour("muted", ` · ${timeline.title}`);
  // A tree nobody has touched for a while is the one that lies, so its age rides in the head.
  const stale =
    ageMs !== undefined && ageMs >= STALE_AFTER_MS
      ? colour("muted", ` · updated ${ageText(ageMs)}`)
      : "";
  const eta = left === null || left === 0 ? "" : colour("muted", ` · ~${estimateText(left)} left`);
  const head = `${segments(split, HEAD_CELLS, colour)} ${colour("muted", `${formatProgress(split.done)}%`)} · ${String(timeline.tasks.length)} tasks${named}${eta}${stale}`;
  return [head, ...lines(timeline, null, [], colour)];
}
