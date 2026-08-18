// This file runs with Bun.
import type { Api, AssistantMessageEvent, Model } from "@earendil-works/pi-ai";
import { expect, test } from "vitest";
import { createDevinOutput, type DevinOutput } from "../src/stream-output.ts";

const MODEL: Model<Api> = {
  id: "adaptive",
  name: "Adaptive",
  api: "devin-cli-acp",
  provider: "devin",
  baseUrl: "https://app.devin.ai",
  reasoning: true,
  input: ["text", "image"],
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
  contextWindow: 209_600,
  maxTokens: 128_000,
};

async function collect(output: DevinOutput): Promise<AssistantMessageEvent[]> {
  const events: AssistantMessageEvent[] = [];
  for await (const event of output.stream) events.push(event);
  return events;
}

test("streams thinking followed by adjacent text and finishes once", async () => {
  const output: DevinOutput = createDevinOutput(MODEL);
  output.appendThinking("");
  output.appendThinking("think");
  output.appendThinking("ing");
  output.appendText("an");
  output.appendText("swer");
  output.finish();
  output.finish();
  output.appendText("ignored");
  output.appendThinking("ignored");

  const events: AssistantMessageEvent[] = await collect(output);
  expect(events.map((event) => event.type)).toStrictEqual([
    "start",
    "thinking_start",
    "thinking_delta",
    "thinking_delta",
    "thinking_end",
    "text_start",
    "text_delta",
    "text_delta",
    "done",
  ]);
  expect(output.partial.content).toStrictEqual([
    { type: "thinking", thinking: "thinking" },
    { type: "text", text: "answer" },
  ]);
});

test("reports non-Error and aborted Error failures", async () => {
  const failed: DevinOutput = createDevinOutput(MODEL);
  failed.appendThinking("pending");
  failed.fail("broken", false);
  failed.fail("ignored", false);
  const aborted: DevinOutput = createDevinOutput(MODEL);
  aborted.fail(new Error("cancelled"), true);

  expect((await collect(failed)).map((event) => event.type)).toStrictEqual([
    "start",
    "thinking_start",
    "thinking_delta",
    "thinking_end",
    "error",
  ]);
  expect(failed.partial.stopReason).toBe("error");
  expect(failed.partial.errorMessage).toBe("broken");
  await collect(aborted);
  expect(aborted.partial.stopReason).toBe("aborted");
  expect(aborted.partial.errorMessage).toBe("cancelled");
});
