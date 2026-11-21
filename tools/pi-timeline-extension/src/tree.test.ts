// This TypeScript file is executed with Bun.
import { expect, test } from "vitest";
import { ageText, estimateText, renderTimeline } from "./render.ts";
import {
  activeTree,
  addTask,
  EMPTY_BOARD,
  estimate,
  measureOf,
  parseBoard,
  parts,
  progress,
  selectTree,
  startTree,
  updateTask,
  withTree,
  type Board,
  type Timeline,
} from "./tree.ts";

/** Names the role it was given, so a test can read which colour each piece took. */
const named = (role: string, text: string): string => `${role}(${text})`;

const tree = (tasks: Timeline["tasks"] = [], title = ""): Timeline => ({
  id: "tree-1",
  tasks,
  title,
});

const built = (): Timeline => {
  const first = addTask(tree(), { title: "Fix bug" });
  const child = addTask(first.timeline, { parent: "t1", title: "Read code" });
  return addTask(child.timeline, { title: "Ship", weight: 3 }).timeline;
};

test("a task gets an id, an estimate-free start, and a subtask points at its parent", () => {
  const first = addTask(tree(), { title: "Fix bug" });
  expect(first.id).toBe("t1");
  expect(first.timeline.tasks).toStrictEqual([
    {
      done: [0, 1],
      id: "t1",
      mode: "serial",
      note: "",
      parent: null,
      priority: null,
      remaining: null,
      status: "pending",
      title: "Fix bug",
      weight: 1,
    },
  ]);
  const second = addTask(first.timeline, { parent: "t1", title: "Read code", weight: 2 });
  expect(second.timeline.tasks[1]?.parent).toBe("t1");
  expect(second.timeline.tasks[1]?.weight).toBe(2);
  expect(() => addTask(first.timeline, { parent: "t9", title: "Nowhere" })).toThrow(
    "Unknown task t9.",
  );
});

test("an update changes only the named task", () => {
  const timeline = built();
  const updated = updateTask(timeline, "t2", {
    done: [1, 2],
    note: "half way",
    priority: "P1",
    remaining: 15,
    status: "active",
  });
  expect(updated.tasks[1]).toStrictEqual({
    done: [1, 2],
    id: "t2",
    mode: "serial",
    note: "half way",
    parent: "t1",
    priority: "P1",
    remaining: 15,
    status: "active",
    title: "Read code",
    weight: 1,
  });
  expect(updated.tasks[0]).toBe(timeline.tasks[0]);
  expect(() => updateTask(timeline, "t9", { status: "done" })).toThrow("Unknown task t9.");
});

test("progress weights siblings and lets subtasks divide their parent's share", () => {
  expect(progress(tree())).toBe(0);
  const timeline = built();
  expect(progress(timeline)).toBe(0);
  const shipped = updateTask(timeline, "t3", { status: "done" });
  expect(progress(shipped)).toBe(0.75);
  // A finished parent still follows its subtasks: one of one done is the whole share.
  expect(progress(updateTask(shipped, "t2", { status: "done" }))).toBe(1);
  // A running task is as far along as the units it recorded, so half of t3's share counts.
  expect(progress(updateTask(timeline, "t3", { done: [1, 2], status: "active" }))).toBe(0.375);
  expect(progress(updateTask(timeline, "t3", { status: "active" }))).toBe(0);
  expect(progress(updateTask(timeline, "t3", { status: "blocked" }))).toBe(0);
  expect(progress(timeline, "t1")).toBe(0);
});

/** A task with no weight at all, which the reference still divides by. */
const zero = (id: string, parent: string | null): Timeline["tasks"][number] => ({
  done: [0, 1],
  id,
  mode: "serial",
  note: "",
  parent,
  priority: null,
  remaining: null,
  status: "pending",
  title: id,
  weight: 0,
});

test("done units make a leaf partly finished, and unfinished work never reads 100", () => {
  const { timeline } = addTask(tree(), { title: "Ship" });
  // Two of three units is two thirds, which the bar shows as a number below 100.
  expect(progress(updateTask(timeline, "t1", { done: [2, 3], status: "active" }))).toBeCloseTo(
    2 / 3,
  );
  // A leaf that says it is done is done, whatever units it recorded.
  expect(progress(updateTask(timeline, "t1", { done: [1, 3], status: "done" }))).toBe(1);
  // Work that is not finished is capped just below 1, as the reference caps it.
  expect(progress(updateTask(timeline, "t1", { done: [3, 3], status: "active" }))).toBe(0.9999);
});

test("a retired task and its subtree leave the plan, and a paused one is still unfinished", () => {
  const timeline = updateTask(built(), "t3", { status: "done" });
  // Retiring the unfinished parent takes its pending subtask with it.
  const retired = updateTask(timeline, "t1", { status: "retired" });
  expect(progress(retired)).toBe(1);
  expect(parts(retired)).toStrictEqual({ active: 0, blocked: 0, done: 1, pending: 0 });
  // A paused task counts as work not yet done, and its own row says so.
  const paused = updateTask(built(), "t3", { status: "paused" });
  expect(progress(paused)).toBe(0);
  expect(measureOf(paused, "t3").status).toBe("paused");
});

test("a group takes the loudest state of its children", () => {
  const three = addTask(addTask(built(), { title: "Docs" }).timeline, { title: "Review" }).timeline;
  expect(measureOf(updateTask(three, "t2", { status: "paused" }), "t1").status).toBe("paused");
  expect(measureOf(updateTask(three, "t2", { status: "active" }), "t1").status).toBe("active");
  const blocked = updateTask(updateTask(three, "t2", { status: "blocked" }), "t4", {
    status: "active",
  });
  // A blocked child outranks a running one, so the group shows what is in the way.
  expect(measureOf(blocked, "t1").status).toBe("blocked");
  expect(measureOf(three, "t1").status).toBe("pending");
});

test("a group of zero weights reports no progress instead of dividing by zero", () => {
  const timeline = tree([zero("t1", null), zero("t2", "t1")]);
  expect(progress(timeline)).toBe(0);
});

test("estimates add up for serial subtasks and take the longest for parallel ones", () => {
  const timeline = updateTask(updateTask(built(), "t2", { remaining: 10 }), "t3", {
    remaining: 30,
  });
  // Fix bug takes its subtask's estimate, so the tree adds 10 and 30.
  expect(estimate(timeline)).toBe(40);
  const parallel = updateTask(timeline, "t1", { mode: "parallel" });
  // T1's own subtasks run together, so its share is the longest of them; the tree still adds up.
  expect(measureOf(parallel, "t1").remaining).toBe(10);
  expect(estimate(parallel)).toBe(40);
  // One task without an estimate makes the whole tree unknown rather than wrong.
  expect(estimate(updateTask(timeline, "t3", { remaining: null }))).toBeNull();
  expect(estimate(updateTask(timeline, "t3", { status: "done" }))).toBe(10);
  expect(estimateText(1)).toBe("1m");
  expect(estimateText(90)).toBe("1h30m");
  expect(estimateText(120)).toBe("2h");
});

test("the tree reads progress, then each task indented under its parent", () => {
  expect(renderTimeline(tree())).toStrictEqual(["No tasks."]);
  const timeline = updateTask(updateTask(built(), "t2", { status: "done" }), "t3", {
    done: [1, 2],
    note: "waiting on review",
    status: "active",
  });
  expect(renderTimeline(timeline)).toStrictEqual([
    "██████░░░░ 62.50% · 3 tasks",
    "██████ 100.00% · t1 Fix bug",
    "██████ 100.00%   ✔ t2 Read code",
    "███░░░  50.00% ▸ t3 Ship — waiting on review",
  ]);
});

test("a row carries its priority and estimate, and the head its title and estimate", () => {
  const timeline = updateTask(updateTask(built(), "t2", { remaining: 5 }), "t3", {
    done: [1, 2],
    priority: "P0",
    remaining: 25,
    status: "active",
    title: "Ship it",
  });
  const titled = renderTimeline({ ...timeline, title: "Fix intake" });
  expect(titled[0]).toBe("████░░░░░░ 37.50% · 3 tasks · Fix intake · ~30m left");
  expect(titled[3]).toBe("███░░░  50.00% ▸ [P0] t3 Ship it · ~25m");
  // A finished task needs no estimate in its row.
  const done = updateTask(timeline, "t3", { status: "done" });
  expect(renderTimeline(done)[3]).toBe("██████ 100.00% ✔ [P0] t3 Ship it");
});

test("a paused or retired row says so with its own glyph", () => {
  const paused = updateTask(built(), "t2", { status: "paused" });
  expect(renderTimeline(paused)[2]).toBe("░░░░░░   0.00%   ⏸ t2 Read code");
  const retired = updateTask(built(), "t2", { status: "retired" });
  expect(renderTimeline(retired)[2]).toBe("░░░░░░   0.00%   – t2 Read code");
});

test("a stored board is read back, and a sidecar without trees is lifted into one", () => {
  const timeline = built();
  const stored: Board = { active: "tree-1", trees: [timeline, tree([], "Docs")] };
  expect(parseBoard(JSON.stringify(stored))).toStrictEqual(stored);
  // The shape written before trees existed still opens, with the new fields filled in.
  const legacy = parseBoard(
    JSON.stringify({
      tasks: [
        { id: "t1", note: "", parent: null, status: "done", title: "Old", weight: 2 },
        { id: "t2", note: "", parent: null, status: "active", title: "Older", weight: 1 },
      ],
    }),
  );
  expect(legacy.active).toBe("tree-1");
  expect(legacy.trees[0]?.tasks[0]).toStrictEqual({
    done: [1, 1],
    id: "t1",
    mode: "serial",
    note: "",
    parent: null,
    priority: null,
    remaining: null,
    status: "done",
    title: "Old",
    weight: 2,
  });
  expect(legacy.trees[0]?.tasks[1]?.done).toStrictEqual([0, 1]);
  expect(() => parseBoard("null")).toThrow("Invalid timeline state.");
  expect(() => parseBoard('{"tasks":1}')).toThrow("Invalid timeline state.");
  expect(() => parseBoard('{"trees":[]}')).toThrow("Invalid timeline state.");
  expect(() => parseBoard('{"trees":[1]}')).toThrow("Invalid timeline state.");
  expect(() => parseBoard('{"trees":[{"tasks":[]}]}')).toThrow("Invalid timeline state.");
  expect(() => parseBoard('{"trees":[{"id":"a","tasks":[{"id":"t1"}]}]}')).toThrow(
    "Invalid timeline state.",
  );
  expect(() => parseBoard('{"trees":[{"id":"a","tasks":[null]}]}')).toThrow(
    "Invalid timeline state.",
  );
  expect(() => parseBoard('{"trees":[{"id":"a","tasks":1}]}')).toThrow("Invalid timeline state.");
  // A plan without a title keeps an empty one, and an unknown priority is no priority.
  const sparse = parseBoard(
    JSON.stringify({
      trees: [
        {
          id: "a",
          tasks: [
            {
              done: [1, 4],
              id: "t1",
              mode: "parallel",
              note: "waiting",
              parent: null,
              priority: "P9",
              remaining: 12,
              status: "active",
              title: "x",
              weight: 1,
            },
          ],
        },
      ],
    }),
  );
  expect(sparse.trees[0]?.title).toBe("");
  expect(sparse.trees[0]?.tasks[0]).toStrictEqual({
    done: [1, 4],
    id: "t1",
    mode: "parallel",
    note: "waiting",
    parent: null,
    priority: null,
    remaining: 12,
    status: "active",
    title: "x",
    weight: 1,
  });
  // A tree id that is gone falls back to the first plan, so a truncated file still shows.
  const board = parseBoard(JSON.stringify({ active: "gone", trees: [tree([], "Docs")] }));
  expect(board.active).toBe("tree-1");
});

test("a session can hold several plans, and starting one twice reuses it", () => {
  const started = startTree(EMPTY_BOARD, "Fix intake");
  // The plan a session starts with is unnamed and empty, so it takes the first goal's title.
  expect(started.id).toBe("tree-1");
  expect(activeTree(started.board).title).toBe("Fix intake");
  // The same goal reopens its own plan instead of forking the tree.
  expect(startTree(started.board, "Fix intake").board).toStrictEqual(started.board);
  const second = startTree(started.board, "Docs");
  expect(second.board.trees.map((entry) => entry.id)).toStrictEqual(["tree-1", "tree-2"]);
  // A third goal has to make a new plan, since the first one is no longer blank.
  expect(startTree(second.board, "Ship").id).toBe("tree-3");
  const selected = selectTree(second.board, "tree-1");
  expect(activeTree(selected).id).toBe("tree-1");
  expect(() => selectTree(second.board, "tree-9")).toThrow("Unknown tree tree-9.");
  // An active id that is gone shows the first plan rather than nothing.
  expect(activeTree({ active: "gone", trees: second.board.trees }).id).toBe("tree-1");
});

test("replacing the active tree keeps the other plans", () => {
  const started = startTree(startTree(EMPTY_BOARD, "Fix intake").board, "Docs").board;
  const replaced = withTree(started, addTask(activeTree(started), { title: "Ship" }).timeline);
  expect(replaced.trees.map((entry) => entry.tasks.length)).toStrictEqual([0, 1]);
  expect(replaced.active).toBe("tree-2");
});

test("the head carries the tree's age only once it is worth reading", () => {
  expect(ageText(0)).toBe("1m");
  expect(ageText(59 * 60_000)).toBe("59m");
  expect(ageText(60 * 60_000)).toBe("1h");
  expect(ageText(25 * 3_600_000)).toBe("1d");
  const timeline = built();
  expect(renderTimeline(timeline, undefined, 2 * 60_000)[0]).toBe("░░░░░░░░░░ 0.00% · 3 tasks");
  expect(renderTimeline(timeline, undefined, 42 * 60_000)[0]).toBe(
    "░░░░░░░░░░ 0.00% · 3 tasks · updated 42m",
  );
});

test("the tree's bar is drawn in the states its work is in", () => {
  const child = updateTask(built(), "t2", { status: "done" });
  // A running task says how much of itself is done; weight 3 gives it the larger share.
  const mixed = updateTask(child, "t3", { done: [1, 3], status: "active" });
  expect(parts(mixed)).toStrictEqual({ active: 0.5, blocked: 0, done: 0.5, pending: 0 });
  expect(progress(mixed)).toBe(0.5);
  expect(renderTimeline(mixed, named)[0]).toBe("done(█████)active(░░░░░) muted(50.00%) · 3 tasks");
  const blocked = updateTask(mixed, "t3", { status: "blocked" });
  expect(parts(blocked)).toStrictEqual({ active: 0, blocked: 0.5, done: 0.5, pending: 0 });
  expect(renderTimeline(blocked, named)[0]).toBe(
    "done(█████)blocked(░░░░░) muted(50.00%) · 3 tasks",
  );
});
