// This TypeScript file is executed with Bun.
import type { Api, AssistantMessage, Model } from "@earendil-works/pi-ai";
import type { SessionBeforeCompactEvent } from "@earendil-works/pi-coding-agent";
import { expect, it, vi } from "vitest";
import { guardedCompaction } from "./context-guard.ts";
import { summarizeDurably } from "./durable-summary.ts";
vi.mock("./durable-summary.ts", () => ({ summarizeDurably: vi.fn() }));

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

it("passes the session and model boundary to durable compaction and retains user requests", async () => {
  vi.mocked(summarizeDurably).mockResolvedValueOnce({
    text: "saved handoff",
    usage: RESPONSE.usage,
  });
  const result = await guardedCompaction(EVENT, {
    model: MODEL,
    modelRegistry: { complete: vi.fn() },
    ui: { notify: vi.fn() },
    sessionManager: { getSessionFile: () => "/sessions/active.jsonl" },
  });
  expect(summarizeDurably).toHaveBeenCalledWith(
    expect.objectContaining({
      sessionFile: "/sessions/active.jsonl",
      identity: expect.stringMatching(/firstKeptEntryId/u),
      text: expect.stringMatching(/historical request/u),
    }),
  );
  expect(result).toMatchObject({
    compaction: {
      firstKeptEntryId: "kept",
      summary: expect.stringMatching(/saved handoff[\s\S]*historical request/u),
    },
  });
});
