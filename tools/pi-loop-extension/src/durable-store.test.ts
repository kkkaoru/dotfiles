// This TypeScript file is executed with Bun.
import { execFileSync } from "node:child_process";
import process from "node:process";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { defineDoc, Harness, MemoryStorage } from "@earendil-works/pi-durable";
import { afterEach, expect, it, vi } from "vitest";

import { DurableLoopStore } from "./durable-store.ts";
import { createLoopState } from "./state.ts";

const EMPTY = createLoopState({
  jobs: [],
  nextId: 1,
  paused: false,
  pendingContinuations: [],
  runningContinuation: undefined,
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it("commits before delivery and restores from durable storage, not stale session entries", async () => {
  const storage = new MemoryStorage();
  vi.spyOn(storage, "close").mockResolvedValue();
  const first = await DurableLoopStore.open(storage, vi.fn());
  expect(await first.restore(undefined)).toBeUndefined();
  expect(await first.restore(EMPTY)).toStrictEqual({
    jobs: [],
    nextId: 1,
    paused: false,
    pendingContinuations: [],
    version: 1,
  });
  const mirrored = vi.fn();
  first.save({ ...EMPTY, paused: true }, mirrored);
  const seen: boolean[] = [];
  first.deliver(() => {
    seen.push(true);
  });
  await first.flush();
  expect(seen).toStrictEqual([true]);
  expect(mirrored).toHaveBeenCalledOnce();
  await first.close();
  const second = await DurableLoopStore.open(storage, vi.fn());
  const restored = await second.restore(EMPTY);
  expect(restored?.paused).toBe(true);
  await second.close();
  await second.close();
});

it("snapshots mutable arrays immediately and cancels queued delivery", async () => {
  const store = await DurableLoopStore.open(new MemoryStorage(), vi.fn());
  const pending: string[] = ["saved"];
  store.save({ ...EMPTY, pendingContinuations: pending });
  pending.push("not saved");
  const send = vi.fn();
  store.deliver(send);
  store.cancelDelivery();
  await store.flush();
  expect(send).not.toHaveBeenCalled();
  const restored = await store.restore(undefined);
  expect(restored?.pendingContinuations).toStrictEqual(["saved"]);
  await store.close();
  expect(() => store.save(EMPTY)).toThrow("closed");
});

it("fails closed on a rejected commit and never delivers or acknowledges success", async () => {
  const storage = new MemoryStorage();
  const report = vi.fn();
  const store = await DurableLoopStore.open(storage, report);
  vi.spyOn(storage, "commit").mockRejectedValue(new Error("disk full"));
  const send = vi.fn();
  store.save(EMPTY);
  store.deliver(send);
  await expect(store.flush()).rejects.toThrow("disk full");
  expect(send).not.toHaveBeenCalled();
  expect(report).toHaveBeenCalledOnce();
  await expect(store.close()).rejects.toThrow("disk full");
});

it("uses checkpointed Pi Durable waits and stops the timer when cancelled", async () => {
  vi.useFakeTimers();
  const store = await DurableLoopStore.open(new MemoryStorage(), vi.fn());
  const callback = vi.fn();
  const cancel = store.scheduler?.setInterval(callback, 100);
  await store.flush();
  await vi.advanceTimersByTimeAsync(210);
  expect(callback).toHaveBeenCalledTimes(2);
  expect(store.scheduler?.now()).toBeTypeOf("number");
  if (cancel !== undefined) {
    store.scheduler?.clearInterval(cancel);
  }
  await vi.advanceTimersByTimeAsync(110);
  expect(callback).toHaveBeenCalledTimes(2);
  await store.close();
});

it("reopens a pending durable timer without duplicating its work", async () => {
  vi.useFakeTimers();
  const storage = new MemoryStorage();
  vi.spyOn(storage, "close").mockResolvedValue();
  const first = await DurableLoopStore.open(storage, vi.fn());
  const stale = vi.fn();
  first.scheduler?.setInterval(stale, 100);
  await first.flush();
  await vi.advanceTimersByTimeAsync(40);
  await first.close();
  const second = await DurableLoopStore.open(storage, vi.fn());
  const current = vi.fn();
  second.scheduler?.setInterval(current, 100);
  await second.flush();
  await vi.advanceTimersByTimeAsync(70);
  expect(stale).not.toHaveBeenCalled();
  expect(current).toHaveBeenCalledOnce();
  await second.close();
});

it("an old cancellation cannot cancel a replacement timer", async () => {
  vi.useFakeTimers();
  const store = await DurableLoopStore.open(new MemoryStorage(), vi.fn());
  const old = vi.fn();
  const cancelOld = store.scheduler?.setInterval(old, 100);
  const current = vi.fn();
  store.scheduler?.setInterval(current, 100);
  cancelOld?.();
  await store.flush();
  await vi.advanceTimersByTimeAsync(110);
  expect(old).not.toHaveBeenCalled();
  expect(current).toHaveBeenCalledOnce();
  await store.close();
});

it("suppresses queued sends on shutdown and compromised storage", async () => {
  const store = await DurableLoopStore.open(new MemoryStorage(), vi.fn());
  const send = vi.fn();
  store.deliver(send);
  await store.close();
  expect(send).not.toHaveBeenCalled();
  const failed = await DurableLoopStore.open(new MemoryStorage(), vi.fn());
  failed.fail("lock lost");
  failed.deliver(send);
  await expect(failed.flush()).rejects.toThrow("lock lost");
  await expect(failed.close()).rejects.toThrow("lock lost");
  expect(send).not.toHaveBeenCalled();
});

it("handles an aborted waiting task and can schedule another", async () => {
  vi.useFakeTimers();
  const opened = vi.spyOn(Harness, "open");
  const store = await DurableLoopStore.open(new MemoryStorage(), vi.fn());
  const harness: Harness | undefined = await opened.mock.results[0]?.value;
  if (harness === undefined) {
    throw new Error("missing harness");
  }
  store.scheduler?.setInterval(vi.fn(), 100);
  await store.flush();
  const task = await harness.commit(async (tx) => {
    const page = await tx.scanTasks({}, 1);
    return page.items[0];
  }, BACKGROUND_CONTEXT);
  if (task === undefined) {
    throw new Error("missing task");
  }
  await harness.abortTask(task.id, BACKGROUND_CONTEXT);
  const settled = await harness.waitForTask(task.id, BACKGROUND_CONTEXT);
  expect(settled.state.outcome.status).toBe("aborted");
  const callback = vi.fn();
  store.scheduler?.setInterval(callback, 100);
  await store.flush();
  await vi.advanceTimersByTimeAsync(110);
  expect(callback).toHaveBeenCalledOnce();
  await store.close();
});

it("rejects corrupt durable state rather than importing legacy state again", async () => {
  const opened = vi.spyOn(Harness, "open");
  const store = await DurableLoopStore.open(new MemoryStorage(), vi.fn());
  const harness: Harness | undefined = await opened.mock.results[0]?.value;
  if (harness === undefined) {
    throw new Error("missing harness");
  }
  const doc = defineDoc<{ state: string; timer: null }>({
    kind: "dotfiles.loop",
    version: 1,
    scope: "session",
    initial: () => ({ state: "{}", timer: null }),
  });
  await harness.commit(async (tx) => {
    await tx.doc(doc);
  }, BACKGROUND_CONTEXT);
  await expect(store.restore(EMPTY)).rejects.toThrow("refusing legacy fallback");
  await store.close();
});

it("stops delivery when the durable harness reports a failure", async () => {
  const opened = vi.spyOn(Harness, "open");
  const report = vi.fn();
  const store = await DurableLoopStore.open(new MemoryStorage(), report);
  opened.mock.calls[0]?.[1].onReport?.(new Error("task failed"));
  const send = vi.fn();
  store.deliver(send);
  await expect(store.flush()).rejects.toThrow("task failed");
  expect(send).not.toHaveBeenCalled();
  expect(report).toHaveBeenCalledOnce();
  await expect(store.close()).rejects.toThrow("task failed");
});

it("loads through the Node Pi extension loader with host module aliases", () => {
  const output: string = execFileSync(process.execPath, ["--input-type=module"], {
    cwd: import.meta.dirname,
    encoding: "utf8",
    timeout: 15_000,
    input: `
      import assert from "node:assert/strict";
      import { resolve } from "node:path";
      import { DefaultResourceLoader, SettingsManager } from "@earendil-works/pi-coding-agent";
      globalThis.fetch = () => { throw new Error("Network disabled in loader regression test"); };
      const loader = new DefaultResourceLoader({
        cwd: resolve(".."),
        agentDir: resolve(".."),
        settingsManager: SettingsManager.inMemory({ packages: [], extensions: [] }),
        additionalExtensionPaths: [resolve("../index.ts")],
        noSkills: true, noThemes: true, noPromptTemplates: true, noContextFiles: true,
      });
      await loader.reload();
      const result = loader.getExtensions();
      assert.deepEqual(result.errors, []);
      assert.equal(result.extensions.length, 1);
      console.log("loaded");
    `,
  });
  expect(output.trim()).toBe("loaded");
});
