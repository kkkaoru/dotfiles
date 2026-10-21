// This file runs with Bun.
import { expect, it } from "vitest";
import type { Api, Model } from "@earendil-works/pi-ai";
import { AsyncQueue } from "../src/async-queue.ts";
import type { SessionEvent } from "../src/session.ts";
import { TurnOutput } from "../src/stream-output.ts";
import { drainTurn } from "../src/turn.ts";
import { sdk, textTurn, toolTurn } from "./fake-query.ts";

const model: Model<Api> = {
  id: "claude-haiku-5-5",
  name: "Claude Haiku 5.5",
  api: "claude-agent-sdk",
  provider: "claude-sdk",
  baseUrl: "https://api.anthropic.com",
  reasoning: true,
  input: ["text"],
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
  contextWindow: 1_000_000,
  maxTokens: 128_000,
};

function queueOf(events: SessionEvent[]): AsyncQueue<SessionEvent> {
  const queue = new AsyncQueue<SessionEvent>();
  events.map((event) => queue.push(event));
  return queue;
}

it("stops at a pi tool call and leaves later events queued", async () => {
  const events = queueOf([...toolTurn("toolu_1", "read", { path: "a" }), ...textTurn("later")]);
  const output = new TurnOutput(model);
  expect(await drainTurn({ events, output })).toStrictEqual({
    kind: "toolUse",
    toolIds: ["toolu_1"],
  });
  expect(output.partial.content).toStrictEqual([
    { type: "toolCall", id: "toolu_1", name: "read", arguments: { path: "a" } },
  ]);
  const next = await events.next();
  expect(next.done).toBe(false);
});

it("streams thinking and skips non-pi tools, subagent events and other blocks", async () => {
  const events = queueOf([
    sdk({ type: "system", subtype: "init" }),
    sdk({
      type: "stream_event",
      parent_tool_use_id: "toolu_parent",
      event: { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
    }),
    sdk({
      type: "stream_event",
      parent_tool_use_id: null,
      event: {
        type: "content_block_start",
        index: 0,
        content_block: { type: "thinking", thinking: "" },
      },
    }),
    sdk({
      type: "stream_event",
      parent_tool_use_id: null,
      event: {
        type: "content_block_delta",
        index: 0,
        delta: { type: "thinking_delta", thinking: "hmm" },
      },
    }),
    sdk({
      type: "stream_event",
      parent_tool_use_id: null,
      event: {
        type: "content_block_delta",
        index: 0,
        delta: { type: "signature_delta", signature: "sig" },
      },
    }),
    sdk({
      type: "stream_event",
      parent_tool_use_id: null,
      event: {
        type: "content_block_start",
        index: 1,
        content_block: { type: "tool_use", id: "toolu_x", name: "WebSearch", input: {} },
      },
    }),
    sdk({
      type: "stream_event",
      parent_tool_use_id: null,
      event: {
        type: "content_block_delta",
        index: 1,
        delta: { type: "input_json_delta", partial_json: "{}" },
      },
    }),
    sdk({
      type: "stream_event",
      parent_tool_use_id: null,
      event: {
        type: "content_block_start",
        index: 2,
        content_block: { type: "redacted_thinking", data: "x" },
      },
    }),
    sdk({
      type: "stream_event",
      parent_tool_use_id: null,
      event: { type: "message_delta", delta: { stop_reason: "tool_use" }, usage: {} },
    }),
    sdk({ type: "stream_event", parent_tool_use_id: null, event: { type: "message_stop" } }),
    sdk({ type: "stream_event", parent_tool_use_id: null, event: { type: "ping" } }),
    sdk({ type: "result", subtype: "success", is_error: false, result: "" }),
  ]);
  const output = new TurnOutput(model);
  expect(await drainTurn({ events, output })).toStrictEqual({ kind: "stop" });
  expect(output.partial.content).toStrictEqual([{ type: "thinking", thinking: "hmm" }]);
});

it("reports a successful-looking result flagged as an error", async () => {
  const events = queueOf([
    sdk({ type: "result", subtype: "success", is_error: true, result: "API Error: 529" }),
  ]);
  const end = await drainTurn({ events, output: new TurnOutput(model) });
  expect(end.kind).toBe("error");
  expect(end.kind === "error" ? String(end.error) : "").toBe(
    "Error: Claude Agent SDK success: API Error: 529",
  );
});

it("names the plan rate-limit window when a result fails", async () => {
  const events = queueOf([
    sdk({
      type: "rate_limit_event",
      rate_limit_info: { status: "rejected", rateLimitType: "five_hour", resetsAt: 1_792_402_200 },
    }),
    sdk({
      type: "result",
      subtype: "success",
      is_error: true,
      result: "API Error: 400 You're out of extra usage.",
    }),
  ]);
  const end = await drainTurn({ events, output: new TurnOutput(model) });
  expect(end.kind === "error" ? String(end.error) : "").toBe(
    "Error: Claude Agent SDK success: API Error: 400 You're out of extra usage. [plan five_hour: rejected, resets 2026-10-19T09:30:00.000Z]",
  );
});

it("names an unspecified rate limit without a reset time", async () => {
  const events = queueOf([
    sdk({ type: "rate_limit_event", rate_limit_info: { status: "allowed_warning" } }),
    sdk({ type: "result", subtype: "success", is_error: true, result: "API Error: 429" }),
  ]);
  const end = await drainTurn({ events, output: new TurnOutput(model) });
  expect(end.kind === "error" ? String(end.error) : "").toBe(
    "Error: Claude Agent SDK success: API Error: 429 [plan limit: allowed_warning]",
  );
});

it("reports a pump failure and an SDK stream that ends early", async () => {
  const failure = new Error("child exited");
  expect(
    await drainTurn({
      events: queueOf([{ type: "pump_failure", error: failure }]),
      output: new TurnOutput(model),
    }),
  ).toStrictEqual({
    kind: "error",
    error: failure,
  });
  const ended = new AsyncQueue<SessionEvent>();
  ended.end();
  const end = await drainTurn({ events: ended, output: new TurnOutput(model) });
  expect(end.kind === "error" ? String(end.error) : "").toBe(
    "Error: Claude Agent SDK session ended before the turn finished",
  );
});
