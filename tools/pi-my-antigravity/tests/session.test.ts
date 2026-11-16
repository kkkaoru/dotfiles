// This file runs with bun.
import type { TranscriptContext } from "@earendil-works/pi-ai";
import { expect, it } from "vitest";

import {
  conversationSeed,
  requestIndex,
  sessionEnvelope,
  stableSessionId,
  stableUuid,
} from "../src/session.ts";

const user = (text: unknown): Record<string, unknown> => ({
  role: "user",
  content: text,
  timestamp: 1,
});

const assistant = (stopReason = "stop"): Record<string, unknown> => ({
  role: "assistant",
  content: [{ type: "text", text: "hi" }],
  api: "my-antigravity-api",
  provider: "my-antigravity",
  model: "gemini-4-hydrogen",
  usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2, cost: {} },
  stopReason,
  timestamp: 2,
});

const asMessages = (...items: unknown[]): TranscriptContext["messages"] =>
  items as TranscriptContext["messages"];

it("generates deterministic UUID-v5-shaped trajectory ids and int64 session ids", () => {
  const first = stableUuid("antigravity:trajectory:session:abc");
  expect(first).toBe("4e1a98cd-2bdd-5a31-a375-d731729592f9");
  expect(first).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-5[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);

  const sessionId = stableSessionId("antigravity:session:session:abc");
  expect(sessionId).toMatch(/^\d{1,19}$/);
  expect(BigInt(sessionId) <= 0x7fffffffffffffffn).toBe(true);
});

it("prefers pi sessionId and falls back to opening user message", () => {
  const withSession = sessionEnvelope({
    sessionId: "pi-session-1",
    messages: asMessages(user("hello")),
    step: 2,
    isClaude: false,
    isNonGemini: false,
  });
  const resumed = sessionEnvelope({
    sessionId: "pi-session-1",
    messages: asMessages(user("hello"), assistant(), user("again")),
    step: 4,
    isClaude: false,
    isNonGemini: false,
  });
  expect(resumed.trajectoryId).toBe(withSession.trajectoryId);
  expect(resumed.sessionId).toBe(withSession.sessionId);

  expect(conversationSeed(undefined, asMessages(user("first")))).toBe("prompt:first");
  expect(conversationSeed("  ", asMessages(user([{ type: "text", text: "hi" }])))).toBe(
    'prompt:[{"type":"text","text":"hi"}]',
  );
  expect(conversationSeed(undefined, asMessages())).toBe("prompt:empty");
});

it("counts completed assistant turns and builds first vs subsequent envelopes", () => {
  expect(requestIndex(asMessages(user("a")))).toBe(0);
  expect(requestIndex(asMessages(user("a"), assistant()))).toBe(1);
  expect(requestIndex(asMessages(user("a"), assistant(), user("b"), assistant()))).toBe(2);
  expect(requestIndex(asMessages(user("a"), assistant("error"), assistant("aborted")))).toBe(0);

  const firstTurn = sessionEnvelope({
    sessionId: "s",
    messages: asMessages(user("hello")),
    step: 2,
    isClaude: false,
    isNonGemini: false,
    modelEnum: "MODEL_PLACEHOLDER_M526",
  });
  expect(firstTurn.labels["last_step_index"]).toBe("1");
  expect(firstTurn.labels["request_id"]).toBe("183baae5-e10d-5d16-97cc-0368de5d6e63-0");
  expect(firstTurn.labels["model_enum"]).toBe("MODEL_PLACEHOLDER_M526");
  expect(firstTurn.labels["last_execution_id"]).toBeUndefined();
  expect(firstTurn.labels["used_claude"]).toBe("false");
  expect(firstTurn.labels["used_non_gemini_model"]).toBe("false");

  const secondTurn = sessionEnvelope({
    sessionId: "s",
    messages: asMessages(user("hello"), assistant(), user("next")),
    step: 4,
    isClaude: true,
    isNonGemini: true,
  });
  expect(secondTurn.labels["request_id"]).toBe("183baae5-e10d-5d16-97cc-0368de5d6e63-1");
  expect(secondTurn.labels["last_step_index"]).toBe("3");
  expect(secondTurn.labels["last_execution_id"]).toBeDefined();
  expect(secondTurn.labels["used_claude"]).toBe("true");
  expect(secondTurn.labels["used_non_gemini_model"]).toBe("true");
});
