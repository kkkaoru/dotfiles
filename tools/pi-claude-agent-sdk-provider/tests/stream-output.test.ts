// This file runs with Bun.
import { expect, it } from "vitest";
import type { Api, AssistantMessageEvent, Model } from "@earendil-works/pi-ai";
import { TurnOutput } from "../src/stream-output.ts";

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

async function eventTypes(output: TurnOutput): Promise<string[]> {
  const types: string[] = [];
  for await (const event of output.stream) {
    types.push(event.type);
  }
  return types;
}

async function lastEvent(output: TurnOutput): Promise<AssistantMessageEvent | undefined> {
  const events: AssistantMessageEvent[] = [];
  for await (const event of output.stream) {
    events.push(event);
  }
  return events.at(-1);
}

it("emits balanced thinking, text and tool-call events", async () => {
  const output = new TurnOutput(model);
  output.appendThinking("plan");
  output.appendThinking(" more");
  output.appendText("Hi");
  output.appendText(" there");
  output.closeBlock();
  output.appendText("Again");
  output.startToolCall("toolu_1", "bash");
  output.appendToolInput('{"command":');
  output.appendToolInput('"ls"}');
  output.closeBlock();
  output.setUsage({ input: 1, output: 2, cacheRead: 3, cacheWrite: 4 });
  output.finish("toolUse");
  expect(await eventTypes(output)).toStrictEqual([
    "start",
    "thinking_start",
    "thinking_delta",
    "thinking_delta",
    "thinking_end",
    "text_start",
    "text_delta",
    "text_delta",
    "text_end",
    "text_start",
    "text_delta",
    "text_end",
    "toolcall_start",
    "toolcall_delta",
    "toolcall_delta",
    "toolcall_end",
    "done",
  ]);
  expect(output.partial.content).toStrictEqual([
    { type: "thinking", thinking: "plan more" },
    { type: "text", text: "Hi there" },
    { type: "text", text: "Again" },
    { type: "toolCall", id: "toolu_1", name: "bash", arguments: { command: "ls" } },
  ]);
  expect(output.partial.usage).toStrictEqual({
    input: 1,
    output: 2,
    cacheRead: 3,
    cacheWrite: 4,
    totalTokens: 10,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
  });
  expect(output.partial.stopReason).toBe("toolUse");
});

it("ignores empty deltas, stray tool input and output after finishing", async () => {
  const output = new TurnOutput(model);
  output.appendText("");
  output.appendThinking("");
  output.appendToolInput("{}");
  output.closeBlock();
  output.finish("stop");
  output.appendText("late");
  output.appendThinking("late");
  output.finish("stop");
  output.fail(new Error("late"), false);
  expect(await eventTypes(output)).toStrictEqual(["start", "done"]);
  expect(output.finished).toBe(true);
});

it("reports an error with its message", async () => {
  const output = new TurnOutput(model);
  output.appendText("partial");
  output.fail(new Error("boom"), false);
  const event = await lastEvent(output);
  expect(event?.type).toBe("error");
  expect(output.partial.stopReason).toBe("error");
  expect(output.partial.errorMessage).toBe("boom");
});

it("reports an abort with a non-Error reason", async () => {
  const output = new TurnOutput(model);
  output.fail("stopped", true);
  await lastEvent(output);
  expect(output.partial.stopReason).toBe("aborted");
  expect(output.partial.errorMessage).toBe("stopped");
});
