// This TypeScript file is executed with Bun.
import type { MarkdownTransformContext, SessionEntry } from "@earendil-works/pi-coding-agent";
import { expect, test } from "vitest";
import {
  createRuntime,
  displayOf,
  processCaches,
  scheduleReply,
  startSession,
  type UiSink,
} from "./runtime.ts";
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

function makeMessageEntry(index: number, lines = 1): SessionEntry {
  const text = Array.from(
    { length: lines },
    (_unused, line) => `Turn ${String(index + 1)} line ${String(line + 1)}`,
  ).join("\n");
  return {
    type: "message",
    id: `m${String(index + 1)}`,
    parentId: index === 0 ? null : `m${String(index)}`,
    timestamp: "2026-01-01T00:00:00.000Z",
    message: {
      role: "assistant",
      content: [{ type: "text", text }],
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

test("a session that opens with history translates about three screens of it", () => {
  // Twenty lines each: the newest six fill the 120-line budget and the older four stay as written.
  const entries = Array.from({ length: 10 }, (_unused, index) => makeMessageEntry(index, 20));
  const server: Server = {
    complete: async (text) => `JA:${text}`,
    ensure: async () => true,
  };
  const box = createRuntime(settings, server, neverResolve);
  startSession(box, {
    ui: ui(),
    sessionManager: { buildContextEntries: () => entries },
  });
  expect(displayOf(box, "Turn 1 line 1", { ...shown, messageId: "m1" })).toBe("Turn 1 line 1");
  expect(displayOf(box, "Turn 4 line 1", { ...shown, messageId: "m4" })).toBe("Turn 4 line 1");
  expect(box.pending.size).toBe(0);

  expect(displayOf(box, "Turn 10 line 1", { ...shown, messageId: "m10" })).toBe("Turn 10 line 1");
  expect(box.pending.has("Turn 10 line 1")).toBe(true);

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
  const long = Array.from({ length: 257 }, (_unused, index) => `Line ${String(index)}`).join("\n");
  expect(displayOf(box, long, { ...shown, messageId: "m1" })).toBe(long);
  expect(box.pending.size).toBe(0);
});

test("a reply over the 128 request budget is left as written", () => {
  const server: Server = {
    complete: async (text) => `JA:${text}`,
    ensure: async () => true,
  };
  const box = createRuntime(settings, server, neverResolve);
  const huge = Array.from({ length: 2564 }, (_unused, index) => `Line ${String(index)}`).join("\n");
  scheduleReply(box, { role: "assistant", content: [{ type: "text", text: huge }] });
  expect(box.pending.size).toBe(0);
  expect(box.progress.total).toBe(0);

  const atLimit = Array.from({ length: 256 }, (_unused, index) => `Line ${String(index)}`).join(
    "\n",
  );
  scheduleReply(box, { role: "assistant", content: [{ type: "text", text: atLimit }] });
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

test("a reload keeps every translation it already made", async () => {
  const caches = processCaches();
  const entries = Array.from({ length: 4 }, (_unused, index) => makeMessageEntry(index));
  const server: Server = { complete: async (text) => `JA:${text}`, ensure: async () => true };

  const before = createRuntime(settings, server, neverResolve, caches);
  scheduleReply(before, { content: [{ type: "text", text: "Older reply" }], role: "assistant" });
  await before.pending.get("Older reply");
  expect(displayOf(before, "Older reply", { ...shown, messageId: "m1" })).toBe("JA:Older reply");

  // The reload builds a new runtime with the same process caches and redraws the whole transcript.
  const after = createRuntime(settings, server, neverResolve, caches);
  startSession(after, { ui: ui(), sessionManager: { buildContextEntries: () => entries } });
  expect(displayOf(after, "Older reply", { ...shown, messageId: "m1" })).toBe("JA:Older reply");
  // The cached entry costs nothing, so the budget still reaches the one never translated before.
  expect(after.pending.size).toBe(0);
  expect(displayOf(after, "Never translated", { ...shown, messageId: "m2" })).toBe(
    "Never translated",
  );
  expect(after.pending.has("Never translated")).toBe(true);
});

test("a reload asks only for the window the budget allows", () => {
  const entries = Array.from({ length: 10 }, (_unused, index) => makeMessageEntry(index, 20));
  const box = createRuntime(settings, working, neverResolve, processCaches());
  startSession(box, { ui: ui(), sessionManager: { buildContextEntries: () => entries } });
  // Drawing the whole transcript asks for at most the budget's worth, newest first.
  for (let index = 0; index < entries.length; index += 1) {
    displayOf(box, `Turn ${String(index + 1)} line 1`, {
      ...shown,
      messageId: `m${String(index + 1)}`,
    });
  }
  expect(box.pending.size).toBe(6);
});

test("the process caches are the same maps on every call", () => {
  expect(processCaches()).toBe(processCaches());
  expect(processCaches().ready).toBe(processCaches().ready);
});
