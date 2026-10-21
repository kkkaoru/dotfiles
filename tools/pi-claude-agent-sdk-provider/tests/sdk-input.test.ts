// This file runs with Bun.
import { expect, it } from "vitest";
import type { Message } from "@earendil-works/pi-ai";
import { initialSdkMessage, userMessagesToSdk } from "../src/sdk-input.ts";

it("sends a lone user message natively with its images", () => {
  const messages: Message[] = [
    { role: "system", content: "sys", timestamp: 1 },
    {
      role: "user",
      content: [
        { type: "text", text: "look" },
        { type: "image", data: "AAAA", mimeType: "image/png" },
      ],
      timestamp: 2,
    },
  ];
  expect(initialSdkMessage(messages)).toStrictEqual({
    type: "user",
    message: {
      role: "user",
      content: [
        { type: "text", text: "look" },
        { type: "image", source: { type: "base64", media_type: "image/png", data: "AAAA" } },
      ],
    },
    parent_tool_use_id: null,
  });
});

it("replaces an image type the API cannot accept with a note", () => {
  expect(
    userMessagesToSdk(
      [
        {
          role: "user",
          content: [{ type: "image", data: "AAAA", mimeType: "image/bmp" }],
          timestamp: 1,
        },
      ],
      [],
    ).message.content,
  ).toStrictEqual([{ type: "text", text: "[unsupported image type: image/bmp]" }]);
});

it("merges queued user messages into one SDK message", () => {
  expect(
    userMessagesToSdk(
      [
        { role: "user", content: "first", timestamp: 1 },
        { role: "user", content: "second", timestamp: 2 },
      ],
      [],
    ).message.content,
  ).toStrictEqual([
    { type: "text", text: "first" },
    { type: "text", text: "second" },
  ]);
});

it("replays a longer history as a text transcript", () => {
  const messages: Message[] = [
    { role: "user", content: "list files", timestamp: 1 },
    {
      role: "assistant",
      content: [
        { type: "thinking", thinking: "hidden" },
        { type: "text", text: "Listing." },
        { type: "toolCall", id: "toolu_1", name: "bash", arguments: { command: "ls" } },
      ],
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
      timestamp: 2,
    },
    {
      role: "toolResult",
      toolCallId: "toolu_1",
      toolName: "bash",
      content: [
        { type: "text", text: "a.txt" },
        { type: "image", data: "AAAA", mimeType: "image/png" },
      ],
      isError: false,
      timestamp: 3,
    },
    {
      role: "toolResult",
      toolCallId: "toolu_2",
      toolName: "read",
      content: [],
      isError: true,
      timestamp: 4,
    },
  ];
  expect(initialSdkMessage(messages).message.content).toStrictEqual([
    {
      type: "text",
      text: [
        "USER:\nlist files",
        'ASSISTANT:\nListing.\n[tool call toolu_1: bash]\n{"command":"ls"}',
        "TOOL RESULT (bash, toolu_1, success):\na.txt\n[image: image/png]",
        "TOOL RESULT (read, toolu_2, error):\n",
        "Continue from the transcript above. Earlier tool calls already ran; follow the latest request.",
      ].join("\n\n"),
    },
  ]);
});

it("sends system updates natively ahead of the first user input", () => {
  const messages: Message[] = [
    { role: "system", content: "base", timestamp: 1 },
    { role: "user", content: "hi", timestamp: 2 },
    { role: "system", content: "Progress note", timestamp: 3 },
  ];
  expect(initialSdkMessage(messages).message.content).toStrictEqual([
    { type: "text", text: "Progress note" },
    { type: "text", text: "hi" },
  ]);
});

it("renders section updates inside a replayed transcript", () => {
  const messages: Message[] = [
    { role: "system", content: "base", timestamp: 1 },
    { role: "user", content: "hi", timestamp: 2 },
    {
      role: "toolResult",
      toolCallId: "t",
      toolName: "bash",
      content: [{ type: "text", text: "ok" }],
      isError: false,
      timestamp: 3,
    },
    { role: "system", content: "", sections: { goal: "<goal>ship</goal>" }, timestamp: 4 },
  ];
  expect(initialSdkMessage(messages).message.content).toStrictEqual([
    {
      type: "text",
      text: [
        "USER:\nhi",
        "TOOL RESULT (bash, t, success):\nok",
        'SYSTEM UPDATE:\nUpdated system prompt section "goal":\n\n<goal>ship</goal>',
        "Continue from the transcript above. Earlier tool calls already ran; follow the latest request.",
      ].join("\n\n"),
    },
  ]);
});
