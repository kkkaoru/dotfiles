// This TypeScript file is executed with Bun.
import { mkdir } from "node:fs/promises";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { createModels, type AssistantMessage } from "@earendil-works/pi-ai";
import {
  createRegistry,
  defineDoc,
  Harness,
  MemoryStorage,
  type JsonObject,
  type Storage,
} from "@earendil-works/pi-durable";
import { openNodeJsonlStorage } from "@earendil-works/pi-durable/storage/jsonl/node";
import { lock } from "proper-lockfile";
import { expect, it, vi } from "vitest";
import { DurableSummaryStore, summarizeDurably } from "./durable-summary.ts";
import {
  summarizeBounded,
  type SummaryCheckpoint,
  type SummaryCheckpointStore,
} from "./bounded-summary.ts";

const mocks = vi.hoisted(() => ({
  openStorage: vi.fn<(...args: Parameters<typeof openNodeJsonlStorage>) => Promise<Storage>>(),
}));

vi.mock("node:fs/promises", () => ({ mkdir: vi.fn() }));
vi.mock("proper-lockfile", () => ({ lock: vi.fn() }));
vi.mock("@earendil-works/pi-durable/storage/jsonl/node", () => ({
  openNodeJsonlStorage: mocks.openStorage,
}));

interface TestDocument extends JsonObject {
  key: string;
  checkpoint: string;
}
const RESPONSE: AssistantMessage = {
  role: "assistant",
  content: [{ type: "text", text: "Keep the user constraints and finish the tests" }],
  api: "openai-responses",
  provider: "openai",
  model: "test",
  timestamp: 0,
  stopReason: "stop",
  usage: {
    input: 1,
    output: 2,
    cacheRead: 0,
    cacheWrite: 0,
    totalTokens: 3,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
  },
};
const REQUEST = {
  text: "x".repeat(2001),
  contextWindow: 24_000,
  signal: new globalThis.AbortController().signal,
  sessionFile: "/sessions/test.jsonl",
  identity: "model-and-boundary",
};
const CHECKPOINT: SummaryCheckpoint = { text: "handoff", nextChunk: 1, usage: RESPONSE.usage };
const TEST_DOC = defineDoc<TestDocument>({
  kind: "dotfiles.effort.compaction",
  version: 1,
  scope: "session",
  initial: () => ({ key: "", checkpoint: "" }),
});

it("reopens committed segments after failure and skips their provider calls", async () => {
  const storage: MemoryStorage = new MemoryStorage();
  vi.spyOn(storage, "close").mockResolvedValue();
  const release = vi.fn<() => Promise<void>>().mockResolvedValue();
  vi.mocked(lock).mockResolvedValue(release);
  mocks.openStorage.mockResolvedValue(storage);
  const first = vi
    .fn()
    .mockResolvedValueOnce(RESPONSE)
    .mockRejectedValueOnce(new Error("provider unavailable"));
  await expect(summarizeDurably({ ...REQUEST, complete: first })).rejects.toThrow(
    "provider unavailable",
  );
  expect(first).toHaveBeenCalledTimes(2);
  expect(release).toHaveBeenCalledOnce();
  const retry = vi.fn().mockResolvedValue(RESPONSE);
  const result = await summarizeDurably({ ...REQUEST, complete: retry });
  expect(retry).toHaveBeenCalledTimes(2);
  expect(retry.mock.calls[0]?.[0]).toMatch(/previous-summary>\nKeep the user constraints/u);
  expect(result.usage.totalTokens).toBe(9);
  expect(release).toHaveBeenCalledTimes(2);
  expect(mkdir).toHaveBeenCalledWith("/sessions/test.jsonl.effort-durable", {
    recursive: true,
    mode: 0o700,
  });
  expect(openNodeJsonlStorage).toHaveBeenCalledWith(
    "/sessions/test.jsonl.effort-durable",
    BACKGROUND_CONTEXT,
    { fsync: true },
  );
});

it.each([
  { text: "changed source" },
  { contextWindow: 48_000 },
  { identity: "different model or boundary" },
])("invalidates the checkpoint for changed request conditions %j", async (change) => {
  const storage: MemoryStorage = new MemoryStorage();
  vi.spyOn(storage, "close").mockResolvedValue();
  vi.mocked(lock).mockResolvedValue(vi.fn());
  mocks.openStorage.mockResolvedValue(storage);
  await summarizeDurably({ ...REQUEST, complete: vi.fn().mockResolvedValue(RESPONSE) });
  const complete = vi.fn().mockResolvedValue(RESPONSE);
  await summarizeDurably({ ...REQUEST, ...change, complete });
  expect(complete).toHaveBeenCalled();
  expect(complete.mock.calls[0]?.[0]).toMatch(/<previous-summary>\n\n/u);
});

it("does not open durable resources for a non-persistent session or aborted request", async () => {
  const complete = vi.fn().mockResolvedValue(RESPONSE);
  await summarizeDurably({ ...REQUEST, sessionFile: undefined, complete });
  expect(complete).toHaveBeenCalledTimes(3);
  await expect(
    summarizeDurably({ ...REQUEST, signal: globalThis.AbortSignal.abort(), complete }),
  ).rejects.toThrow();
  expect(mkdir).not.toHaveBeenCalled();
  expect(lock).not.toHaveBeenCalled();
});

it("fails closed on lock or storage errors and releases an acquired lock", async () => {
  const complete = vi.fn();
  vi.mocked(lock).mockRejectedValueOnce(new Error("already locked"));
  await expect(summarizeDurably({ ...REQUEST, complete })).rejects.toThrow("already locked");
  const release = vi.fn<() => Promise<void>>().mockResolvedValue();
  vi.mocked(lock).mockResolvedValue(release);
  mocks.openStorage.mockRejectedValueOnce(new Error("corrupt journal"));
  await expect(summarizeDurably({ ...REQUEST, complete })).rejects.toThrow("corrupt journal");
  expect(release).toHaveBeenCalledOnce();
  expect(complete).not.toHaveBeenCalled();
});

it("stops before committing a response if the exclusive lock is compromised", async () => {
  const storage: MemoryStorage = new MemoryStorage();
  mocks.openStorage.mockResolvedValue(storage);
  const release = vi.fn<() => Promise<void>>().mockResolvedValue();
  vi.mocked(lock).mockResolvedValue(release);
  const complete = vi.fn().mockImplementation(async () => {
    vi.mocked(lock).mock.calls[0]?.[1]?.onCompromised?.(new Error("lost lock"));
    return RESPONSE;
  });
  await expect(summarizeDurably({ ...REQUEST, complete })).rejects.toThrow("lost lock");
  expect(complete).toHaveBeenCalledOnce();
  expect(release).toHaveBeenCalledOnce();
});

it("keeps checkpoints isolated by identity and snapshots usage before mutation", async () => {
  const storage: MemoryStorage = new MemoryStorage();
  vi.spyOn(storage, "close").mockResolvedValue();
  const store: DurableSummaryStore = await DurableSummaryStore.open(storage, "a");
  expect(await store.load()).toBeUndefined();
  const checkpoint: SummaryCheckpoint = globalThis.structuredClone(CHECKPOINT);
  const saving: Promise<void> = store.save(checkpoint);
  checkpoint.usage.totalTokens = 999;
  await saving;
  const restored = await store.load();
  expect(restored?.usage.totalTokens).toBe(3);
  await store.close();
  const different: DurableSummaryStore = await DurableSummaryStore.open(storage, "b");
  expect(await different.load()).toBeUndefined();
  await different.close();
});

it.each([
  "{broken",
  "null",
  '{"text":1}',
  JSON.stringify({ ...CHECKPOINT, usage: null }),
  JSON.stringify({ ...CHECKPOINT, usage: { ...RESPONSE.usage, cost: null } }),
  JSON.stringify({ ...CHECKPOINT, usage: { ...RESPONSE.usage, input: -1 } }),
  JSON.stringify({
    ...CHECKPOINT,
    usage: { ...RESPONSE.usage, cost: { ...RESPONSE.usage.cost, total: "bad" } },
  }),
  JSON.stringify({ ...CHECKPOINT, usage: { ...RESPONSE.usage, reasoning: -1 } }),
  JSON.stringify({ ...CHECKPOINT, nextChunk: "bad" }),
])("rejects malformed stored state %s", async (checkpoint) => {
  const storage: MemoryStorage = new MemoryStorage();
  vi.spyOn(storage, "close").mockResolvedValue();
  const harness: Harness = await Harness.open(
    storage,
    { models: createModels(), registry: createRegistry() },
    BACKGROUND_CONTEXT,
  );
  await harness.commit(async (tx) => {
    const doc: TestDocument = await tx.doc(TEST_DOC);
    doc.key = "key";
    doc.checkpoint = checkpoint;
  }, BACKGROUND_CONTEXT);
  await harness.close(BACKGROUND_CONTEXT);
  const store: DurableSummaryStore = await DurableSummaryStore.open(storage, "key");
  await expect(store.load()).rejects.toThrow();
  await store.close();
});

it("does not send another request when a checkpoint commit fails", async () => {
  const complete = vi.fn().mockResolvedValue(RESPONSE);
  const checkpoint: SummaryCheckpointStore = {
    load: vi.fn().mockResolvedValue(undefined),
    save: vi.fn().mockRejectedValue(new Error("disk full")),
  };
  await expect(summarizeBounded({ ...REQUEST, complete, checkpoint })).rejects.toThrow("disk full");
  expect(complete).toHaveBeenCalledOnce();
});

it.each([-1, 0.5, 4, Number.NaN])(
  "rejects invalid segment boundary %s before calling the provider",
  async (nextChunk) => {
    const complete = vi.fn();
    const checkpoint: SummaryCheckpointStore = {
      load: async () => ({ ...CHECKPOINT, nextChunk }),
      save: vi.fn(),
    };
    await expect(summarizeBounded({ ...REQUEST, complete, checkpoint })).rejects.toThrow(
      "Invalid bounded compaction checkpoint",
    );
    expect(complete).not.toHaveBeenCalled();
  },
);

it.each(["", "x".repeat(3001)])(
  "rejects an unusable stored summary before provider calls",
  async (text) => {
    const complete = vi.fn();
    const checkpoint: SummaryCheckpointStore = {
      load: async () => ({ ...CHECKPOINT, text }),
      save: vi.fn(),
    };
    await expect(summarizeBounded({ ...REQUEST, complete, checkpoint })).rejects.toThrow(
      "Invalid bounded compaction checkpoint",
    );
    expect(complete).not.toHaveBeenCalled();
  },
);

it("reuses a finished checkpoint when Pi has not yet saved the compaction", async () => {
  const storage: MemoryStorage = new MemoryStorage();
  vi.spyOn(storage, "close").mockResolvedValue();
  vi.mocked(lock).mockResolvedValue(vi.fn());
  mocks.openStorage.mockResolvedValue(storage);
  await summarizeDurably({ ...REQUEST, complete: vi.fn().mockResolvedValue(RESPONSE) });
  const replay = vi.fn();
  const replayed = await summarizeDurably({ ...REQUEST, complete: replay });
  expect(replayed.usage.totalTokens).toBe(9);
  expect(replay).not.toHaveBeenCalled();
});
