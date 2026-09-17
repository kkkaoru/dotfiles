// This TypeScript file is executed with Bun.
import type { Api, AssistantMessage, Model } from "@earendil-works/pi-ai";
import type { SessionBeforeCompactEvent } from "@earendil-works/pi-coding-agent";
import { expect, it, vi } from "vitest";
import contextGuard, {
  guardedCompaction,
  type GuardContext,
  type GuardHost,
} from "./context-guard.ts";

const MODEL: Model<Api> = {
  api: "openai-responses",
  provider: "openai",
  id: "test",
  name: "test",
  baseUrl: "https://example.invalid",
  reasoning: false,
  input: ["text"],
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
  contextWindow: 24_000,
  maxTokens: 4096,
};
const RESPONSE: AssistantMessage = {
  role: "assistant",
  content: [{ type: "text", text: "summary" }],
  api: "openai-responses",
  provider: "openai",
  model: "test",
  timestamp: 0,
  stopReason: "stop",
  usage: {
    input: 1,
    output: 1,
    cacheRead: 0,
    cacheWrite: 0,
    totalTokens: 2,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
  },
};
const EVENT: SessionBeforeCompactEvent = {
  type: "session_before_compact",
  reason: "overflow",
  willRetry: true,
  branchEntries: [],
  signal: new globalThis.AbortController().signal,
  preparation: {
    firstKeptEntryId: "kept",
    tokensBefore: 100_000,
    messagesToSummarize: [{ role: "user", content: "historical request", timestamp: 0 }],
    turnPrefixMessages: [],
    isSplitTurn: false,
    settings: { enabled: true, reserveTokens: 4096, keepRecentTokens: 2000 },
    fileOps: {
      read: new Set(["read.ts"]),
      written: new Set(["write.ts"]),
      edited: new Set(["edit.ts"]),
    },
  },
};

it("registers the compaction guard", () => {
  const on = vi.fn<GuardHost["on"]>();
  contextGuard({ on });
  expect(on.mock.calls[0]?.[0]).toBe("session_before_compact");
});

it("keeps Pi's small normal compactions and handles no selected model", async () => {
  const complete = vi.fn().mockResolvedValue(RESPONSE);
  const ctx: GuardContext = { model: MODEL, modelRegistry: { complete }, ui: { notify: vi.fn() } };
  expect(await guardedCompaction({ ...EVENT, reason: "manual" }, ctx)).toBeUndefined();
  expect(await guardedCompaction(EVENT, { ...ctx, model: undefined })).toBeUndefined();
  expect(complete).not.toHaveBeenCalled();
});

it("uses bounded compaction for overflow and preserves boundary, files, usage, previous summary and focus", async () => {
  const complete = vi
    .fn<(model: unknown, context: unknown, options?: unknown) => Promise<AssistantMessage>>()
    .mockResolvedValue(RESPONSE);
  const ctx: GuardContext = { model: MODEL, modelRegistry: { complete }, ui: { notify: vi.fn() } };
  const result = await guardedCompaction(
    {
      ...EVENT,
      preparation: { ...EVENT.preparation, previousSummary: "old summary" },
      customInstructions: "Keep next steps",
    },
    ctx,
  );
  expect(result).toMatchObject({
    compaction: {
      summary: "summary",
      firstKeptEntryId: "kept",
      tokensBefore: 100_000,
      usage: { totalTokens: 2 },
      details: { readFiles: ["read.ts"], modifiedFiles: ["write.ts", "edit.ts"] },
    },
  });
  expect(JSON.stringify(complete.mock.calls[0]?.[1])).toMatch(
    /old summary[\s\S]*historical request[\s\S]*Keep next steps/u,
  );
  expect(complete.mock.calls[0]?.[2]).toMatchObject({ cacheRetention: "none", maxTokens: 3000 });
});

it("also guards oversized threshold compaction with split turn prefixes and no previous summary", async () => {
  const complete = vi.fn().mockResolvedValue(RESPONSE);
  const ctx: GuardContext = { model: MODEL, modelRegistry: { complete }, ui: { notify: vi.fn() } };
  const result = await guardedCompaction(
    {
      ...EVENT,
      reason: "threshold",
      preparation: {
        ...EVENT.preparation,
        turnPrefixMessages: [{ role: "user", content: "x".repeat(7000), timestamp: 0 }],
      },
    },
    ctx,
  );
  expect(result).toHaveProperty("compaction");
  expect(complete).toHaveBeenCalledTimes(8);
});

it("saves a recovery summary and warns instead of cancelling histories above 128 chunks", async () => {
  const complete = vi.fn().mockResolvedValue(RESPONSE);
  const notify = vi.fn();
  const result = await guardedCompaction(
    {
      ...EVENT,
      preparation: {
        ...EVENT.preparation,
        messagesToSummarize: [{ role: "user", content: "x".repeat(2_000_000), timestamp: 0 }],
      },
    },
    { model: MODEL, modelRegistry: { complete }, ui: { notify } },
  );
  expect(result).toHaveProperty("compaction");
  expect(result).not.toHaveProperty("cancel");
  expect(complete).toHaveBeenCalledTimes(9);
  expect(notify).toHaveBeenCalledWith(
    expect.stringMatching(/^Emergency recovery compaction:/u),
    "warning",
  );
  expect(result).toMatchObject({
    compaction: {
      firstKeptEntryId: "kept",
      summary: expect.stringMatching(/^Emergency recovery compaction:/u),
    },
  });
});

it("cancels on provider errors instead of falling back to the unbounded native request", async () => {
  const complete = vi
    .fn()
    .mockRejectedValueOnce(new Error("request failed"))
    .mockRejectedValueOnce("network unavailable");
  const notify = vi.fn();
  const ctx: GuardContext = { model: MODEL, modelRegistry: { complete }, ui: { notify } };
  expect(await guardedCompaction(EVENT, ctx)).toStrictEqual({ cancel: true });
  expect(notify).toHaveBeenLastCalledWith("request failed", "error");
  expect(await guardedCompaction(EVENT, ctx)).toStrictEqual({ cancel: true });
  expect(notify).toHaveBeenLastCalledWith("network unavailable", "error");
});
