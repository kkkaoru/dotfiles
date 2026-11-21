// This TypeScript file is executed with Bun.
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { MemoryStorage, type Storage } from "@earendil-works/pi-durable";
import { openNodeJsonlStorage } from "@earendil-works/pi-durable/storage/jsonl/node";
import { afterEach, expect, test, vi } from "vitest";
import { DurableTimelineStore } from "./store.ts";
import { addTask, EMPTY_BOARD, type Board, type Timeline } from "./tree.ts";

const directories: string[] = [];

afterEach(async () => {
  await Promise.all(
    directories.splice(0).map(async (dir) => rm(dir, { force: true, recursive: true })),
  );
});

/** A store on a real directory, reopened from the same files the way a resumed session is. */
async function onDisk(): Promise<{ readonly open: () => Promise<DurableTimelineStore> }> {
  const directory = await mkdtemp(path.join(tmpdir(), "pi-timeline-store-"));
  directories.push(directory);
  return {
    open: async () =>
      DurableTimelineStore.open(
        await openNodeJsonlStorage(directory, BACKGROUND_CONTEXT, { fsync: true }),
        vi.fn(),
      ),
  };
}

const tree = (): Timeline =>
  addTask(addTask({ id: "tree-1", tasks: [], title: "" }, { title: "One" }).timeline, {
    title: "Two",
  }).timeline;

const twoTasks = (): Board => ({ active: "tree-1", trees: [tree()] });

test("a saved tree comes back after the store is reopened", async () => {
  const disk = await onDisk();
  const first = await disk.open();
  expect(await first.restore()).toBeUndefined();
  const { timeline } = addTask({ id: "tree-1", tasks: [], title: "" }, { title: "Fix bug" });
  const board: Board = { active: "tree-1", trees: [timeline] };
  first.save(board);
  await first.flush();
  await first.close();

  const second = await disk.open();
  expect(await second.restore()).toStrictEqual(board);
  await second.close();
});

test("only the last save is what a reopened store reads", async () => {
  const disk = await onDisk();
  const first = await disk.open();
  first.save({
    active: "tree-1",
    trees: [addTask({ id: "tree-1", tasks: [], title: "" }, { title: "One" }).timeline],
  });
  first.save(twoTasks());
  await first.flush();
  await first.close();

  const second = await disk.open();
  const restored = await second.restore();
  expect(restored?.trees[0]?.tasks).toHaveLength(2);
  await second.close();
});

test("a closed store refuses writes, and closing twice is harmless", async () => {
  const active = await DurableTimelineStore.open(new MemoryStorage(), vi.fn());
  await active.close();
  await active.close();
  expect(() => active.save(EMPTY_BOARD)).toThrow("Pi Durable timeline store is closed.");
});

/** The same storage, except that every commit fails the way a full disk would. */
function failing(reason: unknown): Storage {
  const storage = new MemoryStorage();
  return new Proxy(storage, {
    get: (target, property, receiver) => {
      if (property === "commit") {
        // The store must handle a rejection that is not an Error.
        return async () => {
          throw reason;
        };
      }
      const value: unknown = Reflect.get(target, property, receiver);
      return typeof value === "function" ? (value as () => unknown).bind(target) : value;
    },
  });
}

test("a failed save is reported once, and flush reports it too", async () => {
  const report = vi.fn();
  const active = await DurableTimelineStore.open(failing(new Error("disk full")), report);
  active.save(twoTasks());
  await expect(active.flush()).rejects.toThrow();
  expect(report).toHaveBeenCalledTimes(1);
  // Once a save has failed the queued ones are dropped instead of retried.
  active.save(EMPTY_BOARD);
  await expect(active.flush()).rejects.toThrow("disk full");
  expect(report).toHaveBeenCalledTimes(1);
});

test("a rejection that is not an Error is reported as one", async () => {
  const report = vi.fn();
  const active = await DurableTimelineStore.open(failing("disk full"), report);
  active.save(twoTasks());
  await expect(active.flush()).rejects.toThrow("disk full");
  expect(report).toHaveBeenCalledWith("disk full");
});
