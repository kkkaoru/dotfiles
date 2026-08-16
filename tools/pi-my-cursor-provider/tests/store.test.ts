import { mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { JsonlLocalAgentStore } from "@cursor/sdk";
import { afterEach, expect, test, vi } from "vitest";

vi.mock("node:fs", async (importOriginal) => {
  const original = await importOriginal<typeof import("node:fs")>();
  return {
    ...original,
    mkdirSync: vi.fn(),
  };
});

afterEach(async () => {
  const { cursorStoreTestApi } = await import("../src/store.ts");
  cursorStoreTestApi.reset();
  vi.mocked(mkdirSync).mockReset();
});

test("places the process store under a pid-specific cache directory", async () => {
  const { cursorProcessAgentId, cursorStoreRoot } = await import("../src/store.ts");
  expect(cursorStoreRoot(4242)).toStrictEqual(
    join(homedir(), ".cache", "pi-my-cursor-provider", "agents", "4242"),
  );
  expect(cursorProcessAgentId(4242, "req-1")).toStrictEqual("pi-cursor-4242-req-1");
});

test("creates one Jsonl store per process and reuses it", async () => {
  const { createProcessCursorStore, cursorStoreTestApi, getProcessCursorStore } =
    await import("../src/store.ts");
  const first = createProcessCursorStore(4242);
  const second = getProcessCursorStore();

  expect(first).toBeInstanceOf(JsonlLocalAgentStore);
  expect(second).toBe(first);
  expect(cursorStoreTestApi.processStoreRoot()).toStrictEqual(
    join(homedir(), ".cache", "pi-my-cursor-provider", "agents", "4242"),
  );
  expect(vi.mocked(mkdirSync)).toHaveBeenCalledWith(
    join(homedir(), ".cache", "pi-my-cursor-provider", "agents", "4242"),
    { recursive: true },
  );
});
