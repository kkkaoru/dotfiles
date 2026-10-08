// This TypeScript file is executed with Bun.
import { execFileSync } from "node:child_process";
import process from "node:process";
import { Harness, MemoryStorage } from "@earendil-works/pi-durable";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { afterEach, expect, it, vi } from "vitest";

import { DurableTmuxState } from "./durable-state.ts";
import { ACTIVE_DISPLAY_ENTRY_TYPE } from "./active-display.ts";
import { TMUX_SESSION_ENTRY_TYPE, TMUX_DELIVERED_ENTRY_TYPE } from "./persistence.ts";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it("imports legacy records and commits before mirroring, then restores the durable journal", async () => {
  const storage = new MemoryStorage();
  vi.spyOn(storage, "close").mockResolvedValue();
  const first = await DurableTmuxState.open(storage, vi.fn());
  await first.restore([
    null,
    {},
    { customType: "unrelated", data: {} },
    { customType: ACTIVE_DISPLAY_ENTRY_TYPE, data: { hidden: true } },
  ]);
  const mirrored = vi.fn();
  first.save(TMUX_SESSION_ENTRY_TYPE, { sessionName: "job-1" }, mirrored);
  first.save(TMUX_DELIVERED_ENTRY_TYPE, { sessionName: "job-1" });
  expect(mirrored).not.toHaveBeenCalled();
  await first.flush();
  expect(mirrored).toHaveBeenCalledOnce();
  await first.close();
  await first.close();
  const second = await DurableTmuxState.open(storage, vi.fn());
  expect(await second.restore([])).toStrictEqual([
    { type: "custom", customType: "pi-tmux-active-display-v1", data: { hidden: true } },
    { type: "custom", customType: "pi-tmux-launch-v2", data: { sessionName: "job-1" } },
    { type: "custom", customType: "pi-tmux-delivered-v1", data: { sessionName: "job-1" } },
  ]);
  await second.close();
  expect(() => second.assertHealthy()).toThrow("closed");
});

it("fails closed on storage failure without delivering an acknowledgement", async () => {
  const storage = new MemoryStorage();
  const report = vi.fn();
  const state = await DurableTmuxState.open(storage, report);
  vi.spyOn(storage, "commit").mockRejectedValue(new Error("disk full"));
  const delivered = vi.fn();
  state.save(ACTIVE_DISPLAY_ENTRY_TYPE, {}, delivered);
  await expect(state.flush()).rejects.toThrow("disk full");
  expect(delivered).not.toHaveBeenCalled();
  expect(report).toHaveBeenCalledOnce();
  expect(() => state.assertHealthy()).toThrow("disk full");
  await expect(state.close()).rejects.toThrow("disk full");
});

it("rejects invalid journal identities", async () => {
  const state = await DurableTmuxState.open(new MemoryStorage(), vi.fn());
  expect(() => state.save("unknown", {})).toThrow("Invalid tmux journal entry");
  expect(() => state.save(TMUX_SESSION_ENTRY_TYPE, null)).toThrow("Invalid tmux journal entry");
  await state.close();
});

it("restores a durable reconciliation deadline and cancels without running commands", async () => {
  vi.useFakeTimers();
  const storage = new MemoryStorage();
  vi.spyOn(storage, "close").mockResolvedValue();
  const first = await DurableTmuxState.open(storage, vi.fn());
  const stale = vi.fn();
  first.monitor?.(stale);
  await first.flush();
  await vi.advanceTimersByTimeAsync(20_000);
  await first.close();
  const second = await DurableTmuxState.open(storage, vi.fn());
  const callback = vi.fn();
  const cancel = second.monitor?.(callback);
  await second.flush();
  await vi.advanceTimersByTimeAsync(40_010);
  expect(stale).not.toHaveBeenCalled();
  expect(callback).toHaveBeenCalledOnce();
  cancel?.();
  await vi.advanceTimersByTimeAsync(60_000);
  expect(callback).toHaveBeenCalledOnce();
  await second.close();
});

it("aborts its durable wait and starts a replacement", async () => {
  vi.useFakeTimers();
  const opened = vi.spyOn(Harness, "open");
  const state = await DurableTmuxState.open(new MemoryStorage(), vi.fn());
  const harness: Harness | undefined = await opened.mock.results[0]?.value;
  if (harness === undefined) {
    throw new Error("Missing harness");
  }
  state.monitor?.(vi.fn());
  await state.flush();
  const task = await harness.commit(async (tx) => {
    const page = await tx.scanTasks({}, 1);
    return page.items[0];
  }, BACKGROUND_CONTEXT);
  if (task === undefined) {
    throw new Error("Missing task");
  }
  await harness.abortTask(task.id, BACKGROUND_CONTEXT);
  const settled = await harness.waitForTask(task.id, BACKGROUND_CONTEXT);
  expect(settled.state.outcome.status).toBe("aborted");
  const callback = vi.fn();
  state.monitor?.(callback);
  await state.flush();
  await vi.advanceTimersByTimeAsync(60_010);
  expect(callback).toHaveBeenCalledOnce();
  await state.close();
});

it("stops queued mutations on a reported harness failure", async () => {
  const opened = vi.spyOn(Harness, "open");
  const report = vi.fn();
  const state = await DurableTmuxState.open(new MemoryStorage(), report);
  const mirror = vi.fn();
  state.save(ACTIVE_DISPLAY_ENTRY_TYPE, {}, mirror);
  opened.mock.calls[0]?.[1].onReport?.("lock lost");
  await expect(state.flush()).rejects.toThrow("lock lost");
  expect(mirror).not.toHaveBeenCalled();
  expect(report).toHaveBeenCalledOnce();
  await expect(state.close()).rejects.toThrow("lock lost");
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
