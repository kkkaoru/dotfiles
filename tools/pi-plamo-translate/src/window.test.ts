// This TypeScript file is executed with Bun.
import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import { expect, test } from "vitest";
import { createRuntime } from "./runtime.ts";
import type { Server } from "./server.ts";
import { recentResumedIds } from "./window.ts";

const settings = { replyLanguage: "ja", promptLanguage: "en", port: 18555 };
const server: Server = { complete: async (text) => `JA:${text}`, ensure: async () => true };
const neverResolve = (): Promise<void> => new Promise<void>(() => {});
const box = () => createRuntime(settings, server, neverResolve);
const ids = (entries: readonly SessionEntry[], runtime = box()): ReadonlySet<string> =>
  recentResumedIds(runtime, {
    ui: { notify: () => undefined, setStatus: () => undefined, setWidget: () => undefined },
    sessionManager: { buildContextEntries: () => entries },
  });

const text = (body: string) => [{ type: "text", text: body }];

function message(id: string, content: unknown): SessionEntry {
  return {
    type: "message",
    id,
    parentId: null,
    timestamp: "2026-01-01T00:00:00.000Z",
    message: { role: "assistant", content, timestamp: 0 },
  } as unknown as SessionEntry;
}

const summary = (id: string, body: string): SessionEntry => ({
  type: "compaction",
  id,
  parentId: null,
  timestamp: "2026-01-01T00:00:00.000Z",
  summary: body,
  firstKeptEntryId: id,
  tokensBefore: 0,
});

test("a summary and a branch summary count, other entry kinds do not", () => {
  const entries = [
    summary("c1", "Compacted"),
    { type: "custom", id: "x1", parentId: null, timestamp: "2026-01-01T00:00:00.000Z" },
    { ...summary("b1", "Branched"), type: "branch_summary", fromId: "m1" },
  ] as readonly SessionEntry[];
  expect([...ids(entries)]).toEqual(["b1", "c1"]);
});

test("a message without text blocks costs nothing", () => {
  const entries = [
    message("m1", undefined),
    { ...message("m0", undefined), message: { role: "assistant", timestamp: 0 } } as SessionEntry,
    message("m2", "not a list"),
    message("m3", [{ type: "toolCall", name: "ls", arguments: {} }]),
    message("m4", text("   ")),
    message("m5", [...text("Kept"), { type: "toolCall", name: "ls", arguments: {} }]),
  ];
  expect([...ids(entries)]).toEqual(["m5"]);
});

test("a text already translated or excluded costs nothing", () => {
  const runtime = box();
  runtime.ready.set("Kept", "JA:Kept");
  runtime.excluded.set("Skipped", true);
  const entries = [message("m1", text("Kept")), message("m2", text("Skipped"))];
  expect(ids(entries, runtime).size).toBe(0);
});

test("the budget keeps the newest entries and skips what does not fit", () => {
  const lines = (id: string, count: number) =>
    message(id, text(Array.from({ length: count }, () => "line").join("\n")));
  // Newest first: 100 lines fit and leave room for the single line, while the 30, the 40 and the
  // 200-line entry no longer fit.
  const entries = [
    lines("m1", 200),
    lines("m2", 40),
    lines("m3", 1),
    lines("m4", 30),
    lines("m5", 100),
  ];
  expect([...ids(entries)]).toEqual(["m5", "m3"]);
});

test("an entry that uses the whole budget ends the window", () => {
  const lines = (id: string, count: number) =>
    message(id, text(Array.from({ length: count }, () => "line").join("\n")));
  const entries = [lines("m1", 1), message("m2", text("   ")), lines("m3", 120)];
  expect([...ids(entries)]).toEqual(["m3"]);
});

test("without a session manager nothing is eligible", () => {
  expect(recentResumedIds(box(), { ui: {} as never }).size).toBe(0);
});
