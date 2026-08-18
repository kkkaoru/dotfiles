import type { Api, AssistantMessageEvent, Model } from "@earendil-works/pi-ai";
import { expect, test } from "vitest";
import { createCursorOutput } from "../src/stream-output.ts";

const MODEL: Model<Api> = {
  id: "auto",
  name: "Cursor Auto",
  api: "cursor-agent",
  provider: "cursor",
  baseUrl: "https://cursor.com",
  reasoning: false,
  input: ["text"],
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
  contextWindow: 200_000,
  maxTokens: 32_000,
};

async function collect(
  output: ReturnType<typeof createCursorOutput>,
): Promise<AssistantMessageEvent[]> {
  const events: AssistantMessageEvent[] = [];
  for await (const event of output.stream) events.push(event);
  return events;
}

test("appends adjacent text and ignores empty or post-finish changes", async () => {
  const output = createCursorOutput(MODEL);
  output.appendText("");
  output.appendText("a");
  output.appendText("b");
  output.finish("stop");
  output.appendText("ignored");
  output.appendToolCall({ type: "toolCall", id: "late", name: "late", arguments: {} });
  output.finish("stop");

  const events = await collect(output);
  expect(events.map((event) => event.type)).toStrictEqual([
    "start",
    "text_start",
    "text_delta",
    "text_delta",
    "done",
  ]);
  expect(output.partial.content).toStrictEqual([{ type: "text", text: "ab" }]);
});

test("closes adjacent thinking before reporting non-Error failures", async () => {
  const output = createCursorOutput(MODEL);
  output.appendThinking("");
  output.appendThinking("a");
  output.appendThinking("b");
  output.fail("broken", false);
  output.appendThinking("ignored");
  output.endThinking();
  output.fail("ignored", false);

  const events = await collect(output);
  expect(events.map((event) => event.type)).toStrictEqual([
    "start",
    "thinking_start",
    "thinking_delta",
    "thinking_delta",
    "thinking_end",
    "error",
  ]);
  const thinkingEnd = events.at(-2);
  expect(thinkingEnd?.type === "thinking_end" ? thinkingEnd.content : "").toBe("ab");
  expect(output.partial.stopReason).toBe("error");
  expect(output.partial.errorMessage).toBe("broken");
});

test("marks aborted Error failures", async () => {
  const output = createCursorOutput(MODEL);
  output.fail(new Error("cancelled"), true);

  await collect(output);
  expect(output.partial.stopReason).toBe("aborted");
  expect(output.partial.errorMessage).toBe("cancelled");
});

test("pushes a complete text block and closes active thinking", async () => {
  const output = createCursorOutput(MODEL);
  output.appendThinking("thinking");
  output.appendTextBlock("block");
  output.finish("stop");

  const events = await collect(output);
  expect(events.map((event) => event.type)).toStrictEqual([
    "start",
    "thinking_start",
    "thinking_delta",
    "thinking_end",
    "text_start",
    "text_delta",
    "done",
  ]);
  expect(output.partial.content).toStrictEqual([
    { type: "thinking", thinking: "thinking" },
    { type: "text", text: "block" },
  ]);
});

test("ignores empty or post-finish text blocks", async () => {
  const output = createCursorOutput(MODEL);
  output.appendTextBlock("");
  output.finish("stop");
  output.appendTextBlock("ignored");

  const events = await collect(output);
  expect(events.map((event) => event.type)).toStrictEqual(["start", "done"]);
});
