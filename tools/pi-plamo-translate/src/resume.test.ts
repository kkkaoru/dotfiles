// This TypeScript file is executed with Bun.
import type { MarkdownTransformContext, SessionEntry } from "@earendil-works/pi-coding-agent";
import { expect, test } from "vitest";
import { createRuntime, displayOf, startSession, type UiSink } from "./runtime.ts";
import type { Server } from "./server.ts";

const settings = { replyLanguage: "ja", promptLanguage: "en", port: 18555 };

const shown: MarkdownTransformContext = {
  messageType: "assistant",
  isStreaming: false,
  availableWidth: 80,
};

function ui(): UiSink {
  return {
    notify: () => undefined,
    setStatus: () => undefined,
    setWidget: () => undefined,
  };
}

function makeMessageEntry(index: number): SessionEntry {
  return {
    type: "message",
    id: `m${String(index + 1)}`,
    parentId: index === 0 ? null : `m${String(index)}`,
    timestamp: "2026-01-01T00:00:00.000Z",
    message: {
      role: "assistant",
      content: [{ type: "text", text: `Turn ${String(index + 1)}` }],
      api: "openai-responses",
      provider: "test",
      model: "test",
      usage: {
        input: 0,
        output: 0,
        cacheRead: 0,
        cacheWrite: 0,
        totalTokens: 0,
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
      },
      stopReason: "stop",
      timestamp: 0,
    },
  };
}

const neverResolve = (): Promise<void> => new Promise<void>(() => {});

test.each(["resume", "fork", "reload"] as const)(
  "on %s, only the most recent persisted entries are scheduled for translation",
  (reason) => {
    const entries = Array.from({ length: 8 }, (_unused, index) => makeMessageEntry(index));
    const server: Server = {
      complete: async (text) => `JA:${text}`,
      ensure: async () => true,
    };
    const box = createRuntime(settings, server, neverResolve);
    startSession(
      box,
      {
        ui: ui(),
        sessionManager: { buildContextEntries: () => entries },
      },
      reason,
    );
    // Older entries (m1, m2) are skipped; recent entry (m8) and live renders (no messageId) translate.
    expect(displayOf(box, "Older reply 1", { ...shown, messageId: "m1" })).toBe("Older reply 1");
    expect(displayOf(box, "Older reply 2", { ...shown, messageId: "m2" })).toBe("Older reply 2");
    expect(box.pending.size).toBe(0);

    expect(displayOf(box, "Recent reply", { ...shown, messageId: "m8" })).toBe("Recent reply");
    expect(box.pending.has("Recent reply")).toBe(true);

    expect(displayOf(box, "Live reply", shown)).toBe("Live reply");
    expect(box.pending.has("Live reply")).toBe(true);
  },
);

test("on compaction resume, the compaction summary entry is included in the recent window", () => {
  const entries: SessionEntry[] = [
    {
      type: "compaction",
      id: "c1",
      parentId: null,
      timestamp: "2026-01-01T00:00:00.000Z",
      summary: "Compacted earlier turns",
      firstKeptEntryId: "m1",
      tokensBefore: 1000,
    },
  ];
  const server: Server = {
    complete: async (text) => `JA:${text}`,
    ensure: async () => true,
  };
  const box = createRuntime(settings, server, neverResolve);
  startSession(box, { ui: ui(), sessionManager: { buildContextEntries: () => entries } }, "resume");
  expect(displayOf(box, "Compacted earlier turns", { ...shown, messageId: "c1" })).toBe(
    "Compacted earlier turns",
  );
  expect(box.pending.has("Compacted earlier turns")).toBe(true);
});
