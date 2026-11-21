// This TypeScript file is executed with Bun.
import { expect, test } from "vitest";
import {
  addTask,
  EMPTY_TIMELINE,
  parseTimeline,
  progress,
  renderTimeline,
  updateTask,
  type Timeline,
} from "./tree.ts";

const built = (): Timeline => {
  const first = addTask(EMPTY_TIMELINE, { title: "Fix bug" });
  const child = addTask(first.timeline, { parent: "t1", title: "Read code" });
  return addTask(child.timeline, { title: "Ship", weight: 3 }).timeline;
};

test("a task gets an id and a subtask points at its parent", () => {
  const first = addTask(EMPTY_TIMELINE, { title: "Fix bug" });
  expect(first.id).toBe("t1");
  expect(first.timeline.tasks).toStrictEqual([
    { id: "t1", note: "", parent: null, status: "pending", title: "Fix bug", weight: 1 },
  ]);
  const second = addTask(first.timeline, { parent: "t1", title: "Read code", weight: 2 });
  expect(second.timeline.tasks[1]).toStrictEqual({
    id: "t2",
    note: "",
    parent: "t1",
    status: "pending",
    title: "Read code",
    weight: 2,
  });
  expect(() => addTask(first.timeline, { parent: "t9", title: "Nowhere" })).toThrow(
    "Unknown task t9.",
  );
});

test("an update changes only the named task", () => {
  const timeline = built();
  const updated = updateTask(timeline, "t2", { note: "done in 2 min", status: "done" });
  expect(updated.tasks[1]).toStrictEqual({
    id: "t2",
    note: "done in 2 min",
    parent: "t1",
    status: "done",
    title: "Read code",
    weight: 1,
  });
  expect(updated.tasks[0]).toBe(timeline.tasks[0]);
  expect(() => updateTask(timeline, "t9", { status: "done" })).toThrow("Unknown task t9.");
});

test("progress weights siblings and lets subtasks divide their parent's share", () => {
  expect(progress(EMPTY_TIMELINE)).toBe(0);
  const timeline = built();
  // Fix bug (weight 1, its subtask pending) and Ship (weight 3, pending).
  expect(progress(timeline)).toBe(0);
  const shipped = updateTask(timeline, "t3", { status: "done" });
  expect(progress(shipped)).toBe(0.75);
  // A finished parent still follows its subtasks: one of one done is the whole share.
  expect(progress(updateTask(shipped, "t2", { status: "done" }))).toBe(1);
  // A working task counts half, a blocked one nothing.
  expect(progress(updateTask(timeline, "t3", { status: "active" }))).toBe(0.375);
  expect(progress(updateTask(timeline, "t3", { status: "blocked" }))).toBe(0);
  expect(progress(timeline, "t1")).toBe(0);
});

test("a tree of zero weights reports no progress instead of dividing by zero", () => {
  const timeline: Timeline = {
    tasks: [{ id: "t1", note: "", parent: null, status: "done", title: "x", weight: 0 }],
  };
  expect(progress(timeline)).toBe(0);
});

test("the tree reads progress, then each task indented under its parent", () => {
  expect(renderTimeline(EMPTY_TIMELINE)).toStrictEqual(["No tasks."]);
  const timeline = updateTask(updateTask(built(), "t2", { status: "done" }), "t3", {
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

test("a stored tree is read back, and anything else is refused", () => {
  const timeline = built();
  expect(parseTimeline(JSON.stringify(timeline))).toStrictEqual(timeline);
  expect(() => parseTimeline("null")).toThrow("Invalid timeline state.");
  expect(() => parseTimeline('{"tasks":1}')).toThrow("Invalid timeline state.");
});
