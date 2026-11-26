import type { Api, AssistantMessage, Model, TranscriptContext } from "@earendil-works/pi-ai";
import { describe, expect, it } from "vitest";

import { INSTALL_HINT } from "../src/constants.ts";
import { readInteraction, streamAntigravity } from "../src/stream.ts";

const model = {
  id: "gemini-4-hydrogen",
  api: "my-antigravity-api",
  provider: "my-antigravity",
} as unknown as Model<Api>;

const context = { messages: [{ role: "user", content: "hi" }] } as unknown as TranscriptContext;

const interaction = {
  steps: [
    { type: "thought", content: [{ type: "text", text: "thinking" }] },
    { type: "model_output", content: [{ type: "text", text: "hello" }] },
    { type: "function_call", id: "c1", name: "read", arguments: { path: "a" } },
  ],
  usage: { input_tokens: 100, cached_tokens: 40, output_tokens: 7 },
};

async function collect(
  run: () => Promise<{ stdout: string; stderr: string }>,
  binary = "/fake",
): Promise<unknown[]> {
  const events: unknown[] = [];
  const stream = streamAntigravity(model, context, { binary, run });
  for await (const event of stream) {
    events.push(event);
  }
  return events;
}

function final(events: unknown[]): AssistantMessage {
  for (const event of events) {
    if (typeof event !== "object" || event === null) {
      continue;
    }
    const record = event as { type?: string; message?: AssistantMessage; error?: AssistantMessage };
    if (record.type === "done" && record.message) {
      return record.message;
    }
    if (record.type === "error" && record.error) {
      return record.error;
    }
  }
  throw new Error("the stream produced no final event");
}

describe("readInteraction", () => {
  it("reads text, thoughts, calls and usage", () => {
    const output = readInteraction(interaction);
    expect(output.text).toBe("hello");
    expect(output.thinking).toBe("thinking");
    expect(output.toolCalls).toEqual([{ id: "c1", name: "read", arguments: { path: "a" } }]);
    expect(output.usage).toMatchObject({ input: 60, output: 7, cacheRead: 40, totalTokens: 107 });
  });

  it("unwraps a nested interaction", () => {
    expect(readInteraction({ interaction }).text).toBe("hello");
  });

  it("returns an empty result for anything else", () => {
    const output = readInteraction("nope");
    expect(output.text).toBe("");
    expect(output.toolCalls).toEqual([]);
    expect(output.usage.totalTokens).toBe(0);
  });

  it("skips a nameless call and a malformed content list", () => {
    const output = readInteraction({
      steps: [
        { type: "function_call", id: "x" },
        { type: "model_output", content: "text" },
        { type: "function_call", id: "y", name: "noargs" },
      ],
    });
    expect(output.toolCalls).toEqual([{ id: "y", name: "noargs", arguments: {} }]);
    expect(output.text).toBe("");
  });

  it("ignores steps that are not records", () => {
    expect(readInteraction({ steps: [null, 3] }).toolCalls).toEqual([]);
  });
});

describe("streamAntigravity", () => {
  it("streams text and reports the usage", async () => {
    const events = await collect(async () => ({ stdout: JSON.stringify(interaction), stderr: "" }));
    const message = final(events);
    expect(message.stopReason).toBe("toolUse");
    expect(message.usage.input).toBe(60);
    expect(message.content).toEqual([
      { type: "thinking", thinking: "thinking" },
      { type: "text", text: "hello" },
      { type: "toolCall", id: "c1", name: "read", arguments: { path: "a" } },
    ]);
    const types = events.map((event) => (event as { type: string }).type);
    expect(types).toContain("start");
    expect(types).toContain("text_delta");
    expect(types).toContain("toolcall_end");
  });

  it("names an id-less tool call", async () => {
    const stdout = JSON.stringify({
      steps: [{ type: "function_call", name: "read", arguments: {} }],
    });
    const message = final(await collect(async () => ({ stdout, stderr: "" })));
    expect(message.content[0]).toMatchObject({ id: "call_0_read", name: "read" });
  });

  it("ends with stop for plain text", async () => {
    const stdout = JSON.stringify({
      steps: [{ type: "model_output", content: [{ type: "text", text: "hi" }] }],
    });
    expect(final(await collect(async () => ({ stdout, stderr: "" }))).stopReason).toBe("stop");
  });

  it("fails when the CLI answers nothing usable", async () => {
    const events = await collect(async () => ({ stdout: "{}", stderr: "" }));
    expect(final(events).errorMessage).toContain("neither text nor a function call");
    expect(final(events).stopReason).toBe("error");
  });

  it("fails when the CLI is missing", async () => {
    const message = final(await collect(async () => ({ stdout: "", stderr: "" }), ""));
    expect(message.errorMessage).toBe(INSTALL_HINT);
  });

  it("carries a CLI failure into the message", async () => {
    const message = final(
      await collect(async () => {
        throw new Error("gemini-api exited with 2: nope");
      }),
    );
    expect(message.errorMessage).toBe("gemini-api exited with 2: nope");
  });
});

describe("readInteraction edge shapes", () => {
  it("reports zero usage when no count field is present", () => {
    expect(readInteraction({ steps: [], usage: { other: 1 } }).usage.totalTokens).toBe(0);
  });

  it("skips a call with an empty name", () => {
    expect(readInteraction({ steps: [{ type: "function_call", name: "" }] }).toolCalls).toEqual([]);
  });
});

describe("readInteraction call shapes", () => {
  it("skips a call whose name is not a string", () => {
    expect(readInteraction({ steps: [{ type: "function_call", name: 7 }] }).toolCalls).toEqual([]);
  });
});
