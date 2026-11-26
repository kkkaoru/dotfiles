import type { Message } from "@earendil-works/pi-ai";
import { describe, expect, it } from "vitest";

import { buildBody } from "../src/request.ts";

const messages = [
  {
    role: "system",
    content: "You are helpful.",
    toolsAdded: [{ name: "read", description: "Read a file", parameters: { type: "object" } }],
  },
  { role: "user", content: "hi" },
  {
    role: "user",
    content: [
      { type: "text", text: "look" },
      { type: "image", data: "AAA", mimeType: "image/png" },
    ],
  },
  {
    role: "assistant",
    content: [
      { type: "text", text: "ok" },
      { type: "toolCall", id: "c1", name: "read", arguments: { path: "a" } },
    ],
  },
  {
    role: "toolResult",
    toolCallId: "c1",
    toolName: "read",
    content: [{ type: "text", text: "file body" }],
    isError: false,
  },
] as unknown as Message[];

describe("buildBody", () => {
  const body = buildBody({ model: "gemini-4-hydrogen", messages });

  it("names the model and asks for one non-streamed interaction", () => {
    expect(body.model).toBe("gemini-4-hydrogen");
    expect(body.stream).toBe(false);
  });

  it("sends the system prompt as the leading user step", () => {
    expect(body.input[0]).toEqual({
      type: "user_input",
      content: [{ type: "text", text: "You are helpful." }],
    });
  });

  it("declares the replayed tools as functions", () => {
    expect(body.tools).toEqual([
      {
        type: "function",
        name: "read",
        description: "Read a file",
        parameters: { type: "object" },
      },
    ]);
  });

  it("maps user text, images, assistant output and tool results", () => {
    expect(body.input.slice(1)).toEqual([
      { type: "user_input", content: [{ type: "text", text: "hi" }] },
      {
        type: "user_input",
        content: [
          { type: "text", text: "look" },
          { type: "image", data: "AAA", mime_type: "image/png" },
        ],
      },
      {
        type: "model_output",
        content: [
          { type: "text", text: "ok" },
          { type: "function_call", id: "c1", name: "read", arguments: { path: "a" } },
        ],
      },
      { type: "function_result", call_id: "c1", name: "read", result: "file body" },
    ]);
  });

  it("marks a failed tool result", () => {
    const failed = buildBody({
      model: "m",
      messages: [
        {
          role: "toolResult",
          toolCallId: "c2",
          toolName: "read",
          content: [{ type: "text", text: "nope" }],
          isError: true,
        },
      ] as unknown as Message[],
    });
    expect(failed.input[0]).toEqual({
      type: "function_result",
      call_id: "c2",
      name: "read",
      result: "nope",
      is_error: true,
    });
    expect(failed.tools).toBeUndefined();
  });

  it("skips an assistant message with nothing to replay", () => {
    const thinkingOnly = buildBody({
      model: "m",
      messages: [
        { role: "assistant", content: [{ type: "thinking", thinking: "hm" }] },
      ] as unknown as Message[],
    });
    expect(thinkingOnly.input).toEqual([]);
  });

  it("omits an empty system prompt", () => {
    const plain = buildBody({
      model: "m",
      messages: [{ role: "system", content: "  " }] as unknown as Message[],
    });
    expect(plain.input).toEqual([]);
  });
});

describe("buildBody tool declarations", () => {
  it("declares a tool that has neither description nor parameters", () => {
    const body = buildBody({
      model: "m",
      messages: [{ role: "system", toolsAdded: [{ name: "bare" }] }] as unknown as Message[],
    });
    expect(body.tools).toEqual([{ type: "function", name: "bare" }]);
  });

  it("drops parameters that are not a JSON object", () => {
    const body = buildBody({
      model: "m",
      messages: [
        { role: "system", toolsAdded: [{ name: "odd", parameters: "nope" }] },
      ] as unknown as Message[],
    });
    expect(body.tools).toEqual([{ type: "function", name: "odd" }]);
  });
});

describe("buildBody content blocks", () => {
  it("ignores a block type the API has no step for", () => {
    const body = buildBody({
      model: "m",
      messages: [{ role: "user", content: [{ type: "audio", data: "x" }] }] as unknown as Message[],
    });
    expect(body.input).toEqual([{ type: "user_input", content: [] }]);
  });
});
