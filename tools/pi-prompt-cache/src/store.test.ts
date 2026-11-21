// This TypeScript file is executed with Bun.
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { MemoryStorage, type Storage } from "@earendil-works/pi-durable";
import { openNodeJsonlStorage } from "@earendil-works/pi-durable/storage/jsonl/node";
import { afterEach, expect, test, vi } from "vitest";
import { EMPTY_LEDGER, record, type Ledger } from "./ledger.ts";
import { DurableLedgerStore } from "./store.ts";

const directories: string[] = [];

afterEach(async () => {
  await Promise.all(
    directories.splice(0).map(async (dir) => rm(dir, { force: true, recursive: true })),
  );
});

/** A store on a real directory, reopened from the same files the way a resumed session is. */
async function onDisk(): Promise<{ readonly open: () => Promise<DurableLedgerStore> }> {
  const directory = await mkdtemp(path.join(tmpdir(), "pi-prompt-cache-store-"));
  directories.push(directory);
  return {
    open: async () =>
      DurableLedgerStore.open(
        await openNodeJsonlStorage(directory, BACKGROUND_CONTEXT, { fsync: true }),
        vi.fn(),
      ),
  };
}

const twoRows: Ledger = record(
  record(EMPTY_LEDGER, { at: 1, cached: 100, model: "m", part: null, prompt: 200 }),
  { at: 2, cached: 50, model: "m", part: "tools", prompt: 200 },
);

test("a saved ledger comes back after the store is reopened", async () => {
  const disk = await onDisk();
  const first = await disk.open();
  expect(await first.restore()).toBeUndefined();
  first.save(twoRows);
  await first.flush();
  await first.close();

  const second = await disk.open();
  expect(await second.restore()).toStrictEqual(twoRows);
  await second.close();
});

test("only the last save is what a reopened store reads", async () => {
  const disk = await onDisk();
  const first = await disk.open();
  first.save(EMPTY_LEDGER);
  first.save(twoRows);
  await first.flush();
  await first.close();

  const second = await disk.open();
  const restored = await second.restore();
  expect(restored?.rows).toHaveLength(2);
  await second.close();
});

test("a write that lands after shutdown is dropped, and closing twice is harmless", async () => {
  const active = await DurableLedgerStore.open(new MemoryStorage(), vi.fn());
  await active.close();
  await active.close();
  // A save racing the shutdown has nowhere to go; it must not surface as an error.
  expect(() => active.save(EMPTY_LEDGER)).not.toThrow();
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
  const active = await DurableLedgerStore.open(failing(new Error("disk full")), report);
  active.save(twoRows);
  await expect(active.flush()).rejects.toThrow();
  expect(report).toHaveBeenCalledTimes(1);
  // Once a save has failed the queued ones are dropped instead of retried.
  active.save(EMPTY_LEDGER);
  await expect(active.flush()).rejects.toThrow("disk full");
  expect(report).toHaveBeenCalledTimes(1);
});

test("a rejection that is not an Error is reported as one", async () => {
  const report = vi.fn();
  const active = await DurableLedgerStore.open(failing("disk full"), report);
  active.save(twoRows);
  await expect(active.flush()).rejects.toThrow("disk full");
  expect(report).toHaveBeenCalledWith("disk full");
});
