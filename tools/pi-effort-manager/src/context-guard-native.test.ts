// This TypeScript file is executed with Bun.
import type { Api, Model } from "@earendil-works/pi-ai";
import type { SessionBeforeCompactEvent } from "@earendil-works/pi-coding-agent";
import * as piCompaction from "@earendil-works/pi-coding-agent";
import { expect, it, vi } from "vitest";

vi.mock("@earendil-works/pi-coding-agent", async (importOriginal) => ({
  ...(await importOriginal<typeof piCompaction>()),
  compact: vi.fn(),
}));
import contextGuard, {
  piCompactionFits,
  type GuardContext,
  type GuardHost,
} from "./context-guard.ts";

const MODEL: Model<Api> = {
  api: "openai-responses",
  provider: "openai",
  id: "test",
  name: "test",
  baseUrl: "https://example.invalid",
  reasoning: true,
  input: ["text"],
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
  contextWindow: 24_000,
  maxTokens: 4096,
};
const EVENT: SessionBeforeCompactEvent = {
  type: "session_before_compact",
  reason: "manual",
  willRetry: false,
  branchEntries: [],
  signal: new globalThis.AbortController().signal,
  preparation: {
    firstKeptEntryId: "kept",
    tokensBefore: 100_000,
    messagesToSummarize: [{ role: "user", content: "historical request", timestamp: 0 }],
    turnPrefixMessages: [],
    isSplitTurn: false,
    settings: { enabled: true, reserveTokens: 4096, keepRecentTokens: 2000 },
    fileOps: { read: new Set(), written: new Set(), edited: new Set() },
  },
};

it("summarizes natively at the thinking level Pi holds when compaction starts", async () => {
  const compact = vi.mocked(piCompaction.compact).mockResolvedValue({
    summary: "handoff",
    firstKeptEntryId: "kept",
    tokensBefore: 100_000,
  });
  const on = vi.fn<GuardHost["on"]>();
  contextGuard({ getThinkingLevel: () => "max", on });
  const context: GuardContext = {
    model: MODEL,
    modelRegistry: { complete: vi.fn() },
    ui: { notify: vi.fn() },
  };
  await on.mock.calls[0]?.[1](EVENT, context);
  expect(compact.mock.calls[0]?.[6]).toBe("max");
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
