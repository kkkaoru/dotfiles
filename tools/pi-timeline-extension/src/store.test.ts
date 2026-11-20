// This TypeScript file is executed with Bun.
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { MemoryStorage, type Storage } from "@earendil-works/pi-durable";
import { openNodeJsonlStorage } from "@earendil-works/pi-durable/storage/jsonl/node";
import { afterEach, expect, test, vi } from "vitest";
import { DurableTimelineStore } from "./store.ts";
import { addTask, EMPTY_TIMELINE, type Timeline } from "./tree.ts";

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

const twoTasks = (): Timeline =>
  addTask(addTask(EMPTY_TIMELINE, { title: "One" }).timeline, {
    title: "Two",
  }).timeline;

test("a saved tree comes back after the store is reopened", async () => {
  const disk = await onDisk();
  const first = await disk.open();
  expect(await first.restore()).toBeUndefined();
  const { timeline } = addTask(EMPTY_TIMELINE, { title: "Fix bug" });
  first.save(timeline);
  await first.flush();
  await first.close();

  const second = await disk.open();
  expect(await second.restore()).toStrictEqual(timeline);
  await second.close();
});

test("only the last save is what a reopened store reads", async () => {
  const disk = await onDisk();
  const first = await disk.open();
  first.save(addTask(EMPTY_TIMELINE, { title: "One" }).timeline);
  first.save(twoTasks());
  await first.flush();
  await first.close();

  const second = await disk.open();
  const restored = await second.restore();
  expect(restored?.tasks).toHaveLength(2);
  await second.close();
});

test("a closed store refuses writes, and closing twice is harmless", async () => {
  const active = await DurableTimelineStore.open(new MemoryStorage(), vi.fn());
  await active.close();
  await active.close();
  expect(() => active.save(EMPTY_TIMELINE)).toThrow("Pi Durable timeline store is closed.");
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
  active.save(EMPTY_TIMELINE);
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
