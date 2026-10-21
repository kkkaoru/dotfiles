// This file runs with Bun.
import { expect, it } from "vitest";
import type { AssistantMessage, Message } from "@earendil-works/pi-ai";
import {
  planContinuation,
  sameSessionSettings,
  type SessionSnapshot,
} from "../src/continuation.ts";
import type { SessionSettings } from "../src/session.ts";

const settings: SessionSettings = {
  cwd: "/work",
  effort: "medium",
  model: "claude-haiku-5-5",
  systemPrompt: "sys",
  thinking: true,
  tools: [{ name: "bash", description: "Run", parameters: { type: "object" } }],
};

const reply: AssistantMessage = {
  role: "assistant",
  content: [],
  api: "claude-agent-sdk",
  provider: "claude-sdk",
  model: "claude-haiku-5-5",
  usage: {
    input: 0,
    output: 0,
    cacheRead: 0,
    cacheWrite: 0,
    totalTokens: 0,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
  },
  stopReason: "toolUse",
  timestamp: 20,
};

const history: Message[] = [{ role: "user", content: "go", timestamp: 10 }, reply];

function snapshot(awaitedToolIds: string[]): SessionSnapshot {
  return { awaitedToolIds, closed: false, replyTimestamp: 20, settings };
}

it("continues with tool results that answer every awaited call", () => {
  const messages: Message[] = [
    ...history,
    {
      role: "toolResult",
      toolCallId: "a",
      toolName: "bash",
      content: [],
      isError: false,
      timestamp: 30,
    },
    {
      role: "toolResult",
      toolCallId: "b",
      toolName: "bash",
      content: [],
      isError: false,
      timestamp: 31,
    },
  ];
  const plan = planContinuation(snapshot(["a", "b"]), messages, settings);
  expect(plan.kind).toBe("toolResults");
  expect(plan.kind === "toolResults" ? plan.updates : undefined).toStrictEqual([]);
  expect(
    plan.kind === "toolResults" ? plan.results.map((result) => result.toolCallId) : [],
  ).toStrictEqual(["a", "b"]);
});

it("restarts when a tool result is missing", () => {
  const messages: Message[] = [
    ...history,
    {
      role: "toolResult",
      toolCallId: "a",
      toolName: "bash",
      content: [],
      isError: false,
      timestamp: 30,
    },
  ];
  expect(planContinuation(snapshot(["a", "b"]), messages, settings)).toStrictEqual({
    kind: "restart",
  });
});

it("restarts when steering input is mixed into tool results", () => {
  const messages: Message[] = [
    ...history,
    {
      role: "toolResult",
      toolCallId: "a",
      toolName: "bash",
      content: [],
      isError: false,
      timestamp: 30,
    },
    { role: "user", content: "also this", timestamp: 31 },
  ];
  expect(planContinuation(snapshot(["a"]), messages, settings)).toStrictEqual({ kind: "restart" });
});

it("continues with new user input and its system updates after a finished reply", () => {
  const messages: Message[] = [
    ...history,
    { role: "user", content: "next", timestamp: 30 },
    { role: "system", content: "progress note", timestamp: 31 },
  ];
  expect(planContinuation(snapshot([]), messages, settings)).toStrictEqual({
    kind: "user",
    messages: [{ role: "user", content: "next", timestamp: 30 }],
    updates: [{ role: "system", content: "progress note", timestamp: 31 }],
  });
});

it("finds the reply after an earlier transient system message disappears", () => {
  const messages: Message[] = [
    { role: "system", content: "base", timestamp: 1 },
    ...history,
    { role: "user", content: "next", timestamp: 30 },
  ];
  expect(planContinuation(snapshot([]), messages, settings).kind).toBe("user");
});

it("restarts when only a system update follows a finished reply", () => {
  const messages: Message[] = [...history, { role: "system", content: "changed", timestamp: 30 }];
  expect(planContinuation(snapshot([]), messages, settings)).toStrictEqual({ kind: "restart" });
});

it("restarts without a session, after close, or on a branched transcript", () => {
  const messages: Message[] = [...history, { role: "user", content: "next", timestamp: 30 }];
  expect(planContinuation(undefined, messages, settings)).toStrictEqual({ kind: "restart" });
  expect(planContinuation({ ...snapshot([]), closed: true }, messages, settings)).toStrictEqual({
    kind: "restart",
  });
  expect(
    planContinuation({ ...snapshot([]), replyTimestamp: 99 }, messages, settings),
  ).toStrictEqual({
    kind: "restart",
  });
  expect(planContinuation(snapshot([]), history, settings)).toStrictEqual({ kind: "restart" });
});

it("restarts when fixed session settings change", () => {
  const messages: Message[] = [...history, { role: "user", content: "next", timestamp: 30 }];
  expect(
    planContinuation(snapshot([]), messages, { ...settings, systemPrompt: "other" }),
  ).toStrictEqual({ kind: "restart" });
});

it("treats effort as live while comparing the other settings", () => {
  expect(sameSessionSettings(settings, { ...settings, effort: "max" })).toBe(true);
  expect(sameSessionSettings(settings, { ...settings, model: "claude-opus-5-5" })).toBe(false);
  expect(sameSessionSettings(settings, { ...settings, cwd: "/other" })).toBe(false);
  expect(sameSessionSettings(settings, { ...settings, thinking: false })).toBe(false);
  expect(sameSessionSettings(settings, { ...settings, tools: [] })).toBe(false);
});
