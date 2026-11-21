// This TypeScript file is executed with Bun.
import { expect, test } from "vitest";
import { start } from "./harness.ts";

const text = (result: { content: readonly { text: string }[] }): string =>
  result.content.map((part) => part.text).join("\n");

test("a new goal gets its own plan, and a session with two lists both", async () => {
  const harness = await start();
  await harness.run({ action: "start", title: "Fix intake" });
  expect(text(await harness.run({ action: "add", title: "Read code" }))).toBe(
    "░░░░░░░░░░ 0.00% · 1 tasks · Fix intake\n░░░░░░   0.00% · t1 Read code",
  );
  await harness.run({ action: "start", title: "Docs" });
  // The second plan is empty, so the answer names the plans instead of drawing nothing.
  expect(text(await harness.run({ action: "list" }))).toBe(
    "No tasks.\nTrees: tree-1 Fix intake, tree-2 Docs (active)",
  );
  const selected = await harness.run({ action: "select", tree: "tree-1" });
  expect(text(selected)).toBe(
    "░░░░░░░░░░ 0.00% · 1 tasks · Fix intake\n░░░░░░   0.00% · t1 Read code\nTrees: tree-1 Fix intake (active), tree-2 Docs",
  );
  // A task can be recorded in a plan without switching to it.
  const elsewhere = await harness.run({ action: "add", tree: "tree-2", title: "Write it down" });
  expect(text(elsewhere)).toContain("t1 Read code");
  expect(text(await harness.run({ action: "select", tree: "tree-2" }))).toContain(
    "t1 Write it down",
  );
});

test("a plan needs the field it is started or selected by", async () => {
  const harness = await start();
  await expect(harness.run({ action: "start" })).rejects.toThrow("action start needs a title");
  await expect(harness.run({ action: "select" })).rejects.toThrow("action select needs a tree id");
  await expect(harness.run({ action: "select", tree: "tree-9" })).rejects.toThrow(
    "Unknown tree tree-9.",
  );
});

test("done units must fit inside their total", async () => {
  const harness = await start();
  await harness.run({ action: "add", title: "Fix bug" });
  const rejected = [
    [2, 1],
    [0, 0],
    [-1, 1],
  ].map(async (done) =>
    harness.run({ action: "update", id: "t1", done }).then(
      () => undefined,
      (error: unknown) => (error as Error).message,
    ),
  );
  expect(await Promise.all(rejected)).toStrictEqual([
    "done needs units finished out of units in total, e.g. [1, 3].",
    "done needs units finished out of units in total, e.g. [1, 3].",
    "done needs units finished out of units in total, e.g. [1, 3].",
  ]);
  expect(text(await harness.run({ action: "update", id: "t1", done: [1, 4] }))).toBe(
    "███░░░░░░░ 25.00% · 1 tasks\n██░░░░  25.00% · t1 Fix bug",
  );
});

test("a priority, an estimate and a mode reach the row and the head", async () => {
  const harness = await start();
  await harness.run({ action: "add", title: "Fix bug" });
  const set = await harness.run({
    action: "update",
    done: [1, 4],
    id: "t1",
    mode: "parallel",
    priority: "P1",
    remaining: 20,
    status: "active",
  });
  expect(text(set)).toBe(
    "███░░░░░░░ 25.00% · 1 tasks · ~20m left\n██░░░░  25.00% ▸ [P1] t1 Fix bug · ~20m",
  );
  // Null clears what a later look no longer knows.
  const cleared = await harness.run({
    action: "update",
    id: "t1",
    priority: null,
    remaining: null,
  });
  expect(text(cleared)).toBe("███░░░░░░░ 25.00% · 1 tasks\n██░░░░  25.00% ▸ t1 Fix bug");
});

test("a plan of only retired work leaves the editor alone", async () => {
  const harness = await start();
  await harness.run({ action: "add", title: "Fix bug" });
  await harness.run({ action: "update", id: "t1", status: "retired" });
  expect(harness.setWidget).toHaveBeenLastCalledWith("timeline", undefined, {
    placement: "belowEditor",
  });
});
