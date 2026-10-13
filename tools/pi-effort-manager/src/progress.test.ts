// This TypeScript file is executed with Bun.
import { convertToLlm, type ContextWithSystemEvent } from "@earendil-works/pi-coding-agent";
import { expect, it } from "vitest";
import { injectProgressOpportunity, ProgressTextController } from "./progress.ts";

it("preserves system sections, tool declarations and the actual user request", () => {
  const messages: ContextWithSystemEvent["messages"] = [
    {
      role: "system",
      content: "Follow project instructions",
      sections: { project: "Finish the requested work; respond only in JSON." },
      toolsAdded: [{ name: "read", description: "Read files", parameters: { type: "object" } }],
      timestamp: 1,
    },
    { content: "Fix the bug", role: "user", timestamp: 2 },
    {
      role: "system",
      content: "",
      sections: { project: "Updated project instructions" },
      toolsRemoved: [{ name: "read" }],
      timestamp: 3,
    },
  ];
  const result = injectProgressOpportunity(messages, "effort-change");
  const converted = convertToLlm(result.messages);

  expect(result.messages).toHaveLength(4);
  expect(result.messages.slice(0, 3)).toStrictEqual(messages);
  expect(result.messages[0]).toBe(messages[0]);
  expect(result.messages[2]).toBe(messages[2]);
  expect(messages).toHaveLength(3);
  expect(converted.map((message) => message.role)).toStrictEqual([
    "system",
    "user",
    "system",
    "system",
  ]);
  expect(converted.filter((message) => message.role === "user")).toStrictEqual([
    { content: "Fix the bug", role: "user", timestamp: 2 },
  ]);
  expect(converted.at(-1)).toMatchObject({
    role: "system",
    content: expect.stringMatching(/different reasoning allocation/u),
  });
  expect(converted.at(-1)?.content).toMatch(/existing instructions[\s\S]*take precedence/u);
  expect(converted.at(-1)?.content).toMatch(/not a reason to stop or ask for confirmation/u);
  expect(converted.at(-1)?.content).toMatch(/do not suppress required communication/u);
});

it("keeps tool results in place when scheduling a post-compaction opportunity", () => {
  const messages: ContextWithSystemEvent["messages"] = [
    {
      role: "toolResult",
      toolCallId: "call-1",
      toolName: "read",
      content: [{ type: "text", text: "File contents" }],
      isError: false,
      timestamp: 1,
    },
  ];
  const result = injectProgressOpportunity(messages, "compaction");

  expect(result.messages[0]).toBe(messages[0]);
  expect(convertToLlm(result.messages).map((message) => message.role)).toStrictEqual([
    "toolResult",
    "system",
  ]);
  expect(result.messages.at(-1)).toMatchObject({
    role: "system",
    content: expect.stringMatching(/context was reorganized/u),
  });
});

it("enables effort opportunities independently from compaction and only once", () => {
  const progress: ProgressTextController = new ProgressTextController({
    progressTextOnCompaction: false,
    progressTextOnEffortChange: true,
  });
  progress.schedule("compaction");
  expect(progress.context([])).toBeUndefined();
  progress.schedule("effort-change");
  expect(progress.context([])?.messages.at(-1)).toMatchObject({
    role: "system",
    content: expect.stringMatching(/different reasoning allocation/u),
  });
  expect(progress.context([])).toBeUndefined();
});

it("enables compaction opportunities independently and resets pending state", () => {
  const progress: ProgressTextController = new ProgressTextController({
    progressTextOnCompaction: true,
    progressTextOnEffortChange: false,
  });
  progress.schedule("effort-change");
  expect(progress.context([])).toBeUndefined();
  progress.schedule("compaction");
  expect(progress.context([])?.messages.at(-1)).toMatchObject({
    role: "system",
    content: expect.stringMatching(/context was reorganized/u),
  });
  progress.schedule("compaction");
  progress.reset({
    progressTextOnCompaction: false,
    progressTextOnEffortChange: false,
  });
  expect(progress.context([])).toBeUndefined();
  progress.schedule("effort-change");
  progress.schedule("compaction");
  expect(progress.context([])).toBeUndefined();
});
