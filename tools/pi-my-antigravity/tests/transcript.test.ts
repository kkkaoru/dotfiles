// This file runs with bun.
import type { Tool, TranscriptContext } from "@earendil-works/pi-ai";
import { expect, it } from "vitest";

import { currentSystemPrompt, currentTools } from "../src/transcript.ts";

const messages = (...items: unknown[]): TranscriptContext["messages"] =>
  items as TranscriptContext["messages"];

const tool = (name: string): Tool =>
  ({ name, description: name, parameters: { type: "object", properties: {} } }) as unknown as Tool;

it("replays string, array, and section system messages in order", () => {
  const prompt = currentSystemPrompt(
    messages(
      { role: "system", content: "base prompt", timestamp: 1 },
      { role: "user", content: "ignored", timestamp: 2 },
      {
        role: "system",
        content: [
          { type: "text", text: "from array" },
          { type: "text", text: "   " },
        ],
        timestamp: 3,
      },
      {
        role: "system",
        sections: { rules: "<rules>be brief</rules>", empty: "  ", removed: null },
        timestamp: 4,
      },
    ),
  );
  expect(prompt).toBe("base prompt\n\nfrom array\n\n<rules>be brief</rules>");
  expect(currentSystemPrompt(messages({ role: "user", content: "hi", timestamp: 1 }))).toBe("");
});

it("applies toolsAdded, tools, and toolsRemoved in order", () => {
  const tools = currentTools(
    messages(
      { role: "system", content: "", toolsAdded: [tool("read"), tool("bash")], timestamp: 1 },
      { role: "user", content: "hi", timestamp: 2 },
      {
        role: "system",
        content: "",
        toolsRemoved: [{ name: "bash" }],
        tools: [tool("edit")],
        timestamp: 3,
      },
    ),
  );
  expect(tools.map((entry) => entry.name)).toStrictEqual(["read", "edit"]);
});
