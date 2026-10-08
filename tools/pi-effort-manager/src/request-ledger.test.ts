// This TypeScript file is executed with Bun.
import { Buffer } from "node:buffer";
import type { SessionBeforeCompactEvent } from "@earendil-works/pi-coding-agent";
import { expect, it } from "vitest";
import { retainedUserRequests, summaryWithoutRequests } from "./context-guard.ts";

const PREPARATION: SessionBeforeCompactEvent["preparation"] = {
  firstKeptEntryId: "kept",
  tokensBefore: 100_000,
  messagesToSummarize: [],
  turnPrefixMessages: [],
  isSplitTurn: false,
  settings: { enabled: true, reserveTokens: 4096, keepRecentTokens: 2000 },
  fileOps: { read: new Set(), written: new Set(), edited: new Set() },
};

it("returns no ledger when the model cannot afford the minimum request budget", () => {
  expect(retainedUserRequests(PREPARATION, 256)).toBe("");
});

it("retains text parts but not images from multipart requests", () => {
  expect(
    retainedUserRequests({
      ...PREPARATION,
      messagesToSummarize: [
        {
          role: "user",
          timestamp: 0,
          content: [
            { type: "text", text: "first" },
            { type: "image", data: "ignored", mimeType: "image/png" },
            { type: "text", text: "last" },
          ],
        },
      ],
    }),
  ).toMatch(/"first\\nlast"/u);
});

it("retains original user requests across repeated compactions without archiving tool output", () => {
  const first = retainedUserRequests({
    ...PREPARATION,
    messagesToSummarize: [
      { role: "user", content: "first request", timestamp: 0 },
      {
        role: "toolResult",
        toolCallId: "id",
        toolName: "bash",
        content: [{ type: "text", text: "massive tool output" }],
        isError: false,
        timestamp: 0,
      },
      { role: "user", content: "second request", timestamp: 0 },
    ],
  });
  expect(first).toMatch(/"first request"\n"second request"/u);
  expect(first).not.toMatch(/massive tool output/u);
  expect(
    retainedUserRequests({
      ...PREPARATION,
      previousSummary: `handoff${first}`,
      messagesToSummarize: [{ role: "user", content: "third request", timestamp: 0 }],
    }),
  ).toMatch(/"first request"\n"second request"\n"third request"/u);
});

it("does not confuse literal ledger tags in a user's request with the ledger boundary", () => {
  const first = retainedUserRequests({
    ...PREPARATION,
    messagesToSummarize: [
      {
        role: "user",
        content: "literal <retained-user-requests> and </retained-user-requests>",
        timestamp: 0,
      },
    ],
  });
  expect(summaryWithoutRequests(`handoff${first}`)).toBe("handoff");
  expect(summaryWithoutRequests("handoff <retained-user-requests> example")).toBe(
    "handoff <retained-user-requests> example",
  );
  expect(
    retainedUserRequests({
      ...PREPARATION,
      previousSummary: `handoff${first}`,
      messagesToSummarize: [{ role: "user", content: "latest", timestamp: 0 }],
    }),
  ).toMatch(/literal <retained-user-requests> and <\/retained-user-requests>[\s\S]*"latest"/u);
});

it("bounds Japanese requests for small models without dropping the newest request", () => {
  const requests = retainedUserRequests(
    {
      ...PREPARATION,
      messagesToSummarize: [{ role: "user", content: "🦊日本語".repeat(1200), timestamp: 0 }],
    },
    24_000,
  );
  expect(Buffer.byteLength(requests, "utf8")).toBeLessThan(3100);
  expect(requests).toMatch(/🦊日本語/u);
  expect(requests).toMatch(/request middle truncated; see original session/u);
  expect(requests).not.toMatch(/\uFFFD/u);
});

it("keeps both ends of a long request like Codex's middle truncation", () => {
  const requests = retainedUserRequests({
    ...PREPARATION,
    messagesToSummarize: [
      {
        role: "user",
        content: `initial instruction ${"x".repeat(10_000)} final requirement`,
        timestamp: 0,
      },
    ],
  });
  expect(requests).toMatch(/initial instruction/u);
  expect(requests).toMatch(/final requirement/u);
  expect(requests).toMatch(/request middle truncated; see original session/u);
});

it("retains a valid newest entry even when JSON escaping expands past a tiny budget", () => {
  const requests = retainedUserRequests(
    {
      ...PREPARATION,
      messagesToSummarize: [{ role: "user", content: "\u0000".repeat(1000), timestamp: 0 }],
    },
    1024,
  );
  expect(Buffer.byteLength(requests, "utf8")).toBeLessThan(129);
  expect(requests).toMatch(/request middle truncated; see original session/u);
  expect(requests).toMatch(/<retained-user-requests>\n"/u);
});
