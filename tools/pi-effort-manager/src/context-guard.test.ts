// This TypeScript file is executed with Bun.
import type { Api, AssistantMessage, Model } from "@earendil-works/pi-ai";
import type { SessionBeforeCompactEvent } from "@earendil-works/pi-coding-agent";
import { expect, it, vi } from "vitest";
import contextGuard, {
  guardedCompaction,
  piCompactionFits,
  providerSessionHeaders,
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
  expect(complete.mock.calls[0]?.[2]).toMatchObject({
    cacheRetention: "none",
    maxTokens: 3000,
    reasoning: "low",
    maxRetries: 6,
  });
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

it("keeps Pi's single-call compaction while its summary cap can hold the history", async () => {
  const complete = vi.fn().mockResolvedValue(RESPONSE);
  const ctx: GuardContext = { model: MODEL, modelRegistry: { complete }, ui: { notify: vi.fn() } };
  const result = await guardedCompaction(
    {
      ...EVENT,
      reason: "threshold",
      preparation: {
        ...EVENT.preparation,
        tokensBefore: 200_000,
        messagesToSummarize: [{ role: "user", content: "x".repeat(200_000), timestamp: 0 }],
        settings: { enabled: true, reserveTokens: 65_536, keepRecentTokens: 12_000 },
      },
    },
    { ...ctx, model: { ...MODEL, contextWindow: 600_000, maxTokens: 65_536 } },
  );
  expect(result).toBeUndefined();
  expect(complete).not.toHaveBeenCalled();
});

it("bounds a fitting window when the model's output exceeds the compaction reserve", async () => {
  const complete = vi.fn().mockResolvedValue(RESPONSE);
  const result = await guardedCompaction(
    {
      ...EVENT,
      reason: "threshold",
      preparation: {
        ...EVENT.preparation,
        tokensBefore: 400_000,
        messagesToSummarize: [{ role: "user", content: "x".repeat(50_000), timestamp: 0 }],
        settings: { enabled: true, reserveTokens: 65_536, keepRecentTokens: 12_000 },
      },
    },
    {
      model: { ...MODEL, contextWindow: 600_000, maxTokens: 384_000 },
      modelRegistry: { complete },
      ui: { notify: vi.fn() },
    },
  );
  expect(result).toHaveProperty("compaction");
  expect(complete).toHaveBeenCalled();
});

it("keeps Pi's single call only while its output cap can hold the summary", () => {
  const preparation = {
    ...EVENT.preparation,
    settings: { enabled: true, reserveTokens: 65_536, keepRecentTokens: 12_000 },
  };
  const model = { ...MODEL, contextWindow: 600_000, maxTokens: 65_536 };
  // Pi caps the summary at 0.8 * 65_536 = 52_428 tokens, so an 88k history fits with room to spare.
  expect(piCompactionFits({ ...preparation, tokensBefore: 100_000 }, model)).toBe(true);
  // A 522k history needs ~10x compression past that cap, exactly how Pi's single call hits it.
  expect(piCompactionFits({ ...preparation, tokensBefore: 534_464 }, model)).toBe(false);
  // A model without a declared output limit gets Pi's full reserve share.
  expect(
    piCompactionFits({ ...preparation, tokensBefore: 100_000 }, { ...model, maxTokens: 0 }),
  ).toBe(true);
  // A 128k window with an 8192-token output cap cannot hold the summary of a 58k-token history:
  // Pi's single call spends the whole cap and stops at length, failing the whole compaction.
  const smallCap = { ...MODEL, contextWindow: 128_000, maxTokens: 8192 };
  expect(piCompactionFits({ ...preparation, tokensBefore: 69_761 }, smallCap)).toBe(false);
  expect(piCompactionFits({ ...preparation, tokensBefore: 40_000 }, smallCap)).toBe(true);
});

it("bounds a large-window compaction whose history outgrows Pi's summary cap", async () => {
  const complete = vi.fn().mockResolvedValue(RESPONSE);
  const result = await guardedCompaction(
    {
      ...EVENT,
      reason: "threshold",
      preparation: {
        ...EVENT.preparation,
        tokensBefore: 534_464,
        messagesToSummarize: [{ role: "user", content: "x".repeat(60_000), timestamp: 0 }],
        settings: { enabled: true, reserveTokens: 65_536, keepRecentTokens: 12_000 },
      },
    },
    {
      model: { ...MODEL, contextWindow: 600_000, maxTokens: 65_536 },
      modelRegistry: { complete },
      ui: { notify: vi.fn() },
    },
  );
  expect(result).toHaveProperty("compaction");
  expect(complete).toHaveBeenCalled();
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
  expect(complete).not.toHaveBeenCalled();
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

it("sends the opencode session header that Pi's own requests carry", () => {
  expect(providerSessionHeaders({ ...MODEL, provider: "opencode-go" }, "session-1")).toStrictEqual({
    "x-opencode-session": "session-1",
    "x-opencode-client": "pi",
    "User-Agent": "pi-coding-agent",
  });
  expect(
    providerSessionHeaders(
      { ...MODEL, provider: "custom", baseUrl: "https://opencode.ai/zen" },
      "session-1",
    ),
  ).toStrictEqual({
    "x-opencode-session": "session-1",
    "x-opencode-client": "pi",
    "User-Agent": "pi-coding-agent",
  });
  expect(providerSessionHeaders(MODEL, "session-1")).toStrictEqual({});
  expect(
    providerSessionHeaders({ ...MODEL, provider: "custom", baseUrl: "not a url" }, "session-1"),
  ).toStrictEqual({});
});

it("passes those headers and one session ID to every summary segment", async () => {
  const complete = vi.fn().mockResolvedValue(RESPONSE);
  const result = await guardedCompaction(EVENT, {
    model: { ...MODEL, provider: "opencode-go", baseUrl: "https://opencode.ai/zen" },
    modelRegistry: { complete },
    ui: { notify: vi.fn() },
  });
  expect(result).toHaveProperty("compaction");
  expect(complete.mock.calls[0]?.[2]).toMatchObject({
    headers: {
      "x-opencode-session": expect.any(String),
      "x-opencode-client": "pi",
      "User-Agent": "pi-coding-agent",
    },
    sessionId: expect.any(String),
    maxRetries: 6,
  });
});

it("saves emergency recovery without waiting on provider output", async () => {
  const complete = vi.fn();
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
  expect(complete).not.toHaveBeenCalled();
  expect(notify).toHaveBeenCalledWith(
    expect.stringMatching(/^Emergency recovery compaction:/u),
    "warning",
  );
  expect(result).toMatchObject({
    compaction: {
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
