// This TypeScript file is executed with Bun.
import type { MarkdownTransformContext, SessionEntry } from "@earendil-works/pi-coding-agent";
import { expect, test } from "vitest";
import { createRuntime, displayOf, scheduleReply, startSession, type UiSink } from "./runtime.ts";
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

test("a session that opens with history only translates the most recent persisted entries", () => {
  const entries = Array.from({ length: 8 }, (_unused, index) => makeMessageEntry(index));
  const server: Server = {
    complete: async (text) => `JA:${text}`,
    ensure: async () => true,
  };
  const box = createRuntime(settings, server, neverResolve);
  startSession(box, {
    ui: ui(),
    sessionManager: { buildContextEntries: () => entries },
  });
  // Only the last two entries (m7, m8) translate; older ones stay as written.
  expect(displayOf(box, "Older reply 1", { ...shown, messageId: "m1" })).toBe("Older reply 1");
  expect(displayOf(box, "Older reply 6", { ...shown, messageId: "m6" })).toBe("Older reply 6");
  expect(box.pending.size).toBe(0);

  expect(displayOf(box, "Recent reply", { ...shown, messageId: "m8" })).toBe("Recent reply");
  expect(box.pending.has("Recent reply")).toBe(true);

  expect(displayOf(box, "Live reply", shown)).toBe("Live reply");
  // A live reply is scheduled when it ends, never by drawing it.
  expect(box.pending.has("Live reply")).toBe(false);
});

test("a long persisted entry stays untranslated", () => {
  const entries = [makeMessageEntry(0)];
  const server: Server = {
    complete: async (text) => `JA:${text}`,
    ensure: async () => true,
  };
  const box = createRuntime(settings, server, neverResolve);
  startSession(box, { ui: ui(), sessionManager: { buildContextEntries: () => entries } });
  const long = Array.from({ length: 25 }, (_unused, index) => `Line ${String(index)}`).join("\n");
  expect(displayOf(box, long, { ...shown, messageId: "m1" })).toBe(long);
  expect(box.pending.size).toBe(0);
});

test("a reply long enough to need hundreds of chunks is left as written", () => {
  const server: Server = {
    complete: async (text) => `JA:${text}`,
    ensure: async () => true,
  };
  const box = createRuntime(settings, server, neverResolve);
  const huge = Array.from({ length: 2564 }, (_unused, index) => `Line ${String(index)}`).join("\n");
  scheduleReply(box, { role: "assistant", content: [{ type: "text", text: huge }] });
  expect(box.pending.size).toBe(0);
  expect(box.progress.total).toBe(0);

  scheduleReply(box, { role: "assistant", content: [{ type: "text", text: "One line." }] });
  expect(box.pending.size).toBe(1);
});

test("a compaction summary entry is included in the recent window", () => {
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
  startSession(box, { ui: ui(), sessionManager: { buildContextEntries: () => entries } });
  expect(displayOf(box, "Compacted earlier turns", { ...shown, messageId: "c1" })).toBe(
    "Compacted earlier turns",
  );
  expect(box.pending.has("Compacted earlier turns")).toBe(true);
});

const working: Server = { complete: async (text) => `JA:${text}`, ensure: async () => true };

test("a long transcript is not translated, however often it is drawn", () => {
  const box = createRuntime(settings, working, neverResolve);
  // Drawn before any session start: nothing persisted is eligible.
  const texts = Array.from({ length: 50 }, (_unused, index) => `Reply ${String(index)}`);
  for (let pass = 0; pass < 3; pass += 1) {
    texts.forEach((text, index) => {
      displayOf(box, text, { ...shown, messageId: `m${String(index + 1)}` });
    });
  }
  expect(box.pending.size).toBe(0);
});

test("a session start without a session manager still translates nothing persisted", () => {
  const box = createRuntime(settings, working, neverResolve);
  startSession(box, { ui: ui() });
  expect(displayOf(box, "Reply", { ...shown, messageId: "m1" })).toBe("Reply");
  expect(box.pending.size).toBe(0);
});
