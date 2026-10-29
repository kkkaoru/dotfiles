// This TypeScript file is executed with Bun.
import type { ToolResultMessage } from "@earendil-works/pi-ai";
import { expect, it } from "vitest";
import { branchResults, roundResults, toolResultEvidence } from "./tool-signature.ts";

function result(toolCallId: string, text: string, isError = false): ToolResultMessage {
  return {
    content: [
      { text, type: "text" },
      { data: "", mimeType: "image/png", type: "image" },
    ],
    isError,
    role: "toolResult",
    timestamp: 0,
    toolCallId,
    toolName: "bash",
  };
}

it("pairs results with their call input, ignoring the call's free-text description", () => {
  const message = {
    content: [
      { text: "running", type: "text" },
      {
        arguments: { command: "bun test", description: "Run the tests" },
        id: "a",
        name: "bash",
        type: "toolCall",
      },
      {
        arguments: { command: "bun test", description: "Again" },
        id: "b",
        name: "bash",
        type: "toolCall",
      },
    ],
    role: "assistant",
  };
  expect(roundResults(message, [result("a", "FAIL"), result("b", "FAIL", true)])).toStrictEqual([
    {
      isError: false,
      malformed: false,
      inputKey: 'bash\u000022\u0000{"command":"bun test"}\u0000{"command":"bun test"}',
      signature:
        'bash\u000022\u0000{"command":"bun test"}\u0000{"command":"bun test"}\u0000false\u00004\u0000FAIL\u0000FAIL',
    },
    {
      isError: true,
      malformed: false,
      inputKey: 'bash\u000022\u0000{"command":"bun test"}\u0000{"command":"bun test"}',
      signature:
        'bash\u000022\u0000{"command":"bun test"}\u0000{"command":"bun test"}\u0000true\u00004\u0000FAIL\u0000FAIL',
    },
  ]);
});

it("keeps results without a matching call and ignores malformed blocks", () => {
  const message = {
    content: [
      null,
      { id: 1, name: "bash", type: "toolCall" },
      { id: "c", name: "bash", type: "toolCall" },
    ],
  };
  expect(roundResults(message, [result("c", "ok"), result("missing", "ok")])).toStrictEqual([
    {
      isError: false,
      malformed: false,
      inputKey: "bash\u00000\u0000\u0000",
      signature: "bash\u00000\u0000\u0000\u0000false\u00002\u0000ok\u0000ok",
    },
    {
      isError: false,
      malformed: false,
      inputKey: "bash\u0000",
      signature: "bash\u0000\u0000false\u00002\u0000ok\u0000ok",
    },
  ]);
  expect(roundResults(undefined, [])).toStrictEqual([]);
  expect(roundResults({ content: "text" }, [result("x", "")])).toStrictEqual([
    {
      isError: false,
      malformed: false,
      inputKey: "bash\u0000",
      signature: "bash\u0000\u0000false\u00000\u0000\u0000",
    },
  ]);
});

it("bounds a signature to slices of the input and the result", () => {
  const longText = `${"a".repeat(300)}${"b".repeat(300)}`;
  const message = {
    content: [
      { arguments: { content: "x".repeat(5000) }, id: "w", name: "write", type: "toolCall" },
    ],
  };
  const rounds = roundResults(message, [result("w", longText)]);
  expect(rounds.at(0)?.signature?.length).toBe(1559);
});
it("reads the tool results at the tail of a branch with their calls", () => {
  const entries = [
    { type: "custom" },
    { message: { role: "user" }, type: "message" },
    {
      message: {
        content: [{ arguments: { command: "bun test" }, id: "a", name: "bash", type: "toolCall" }],
        role: "assistant",
      },
      type: "message",
    },
    { message: result("a", "FAIL", true), type: "message" },
    { message: { role: "toolResult", toolCallId: 1 }, type: "message" },
  ];
  expect(branchResults(entries)).toStrictEqual([
    {
      isError: true,
      malformed: false,
      inputKey: 'bash\u000022\u0000{"command":"bun test"}\u0000{"command":"bun test"}',
      signature:
        'bash\u000022\u0000{"command":"bun test"}\u0000{"command":"bun test"}\u0000true\u00004\u0000FAIL\u0000FAIL',
    },
  ]);
  expect(branchResults([])).toStrictEqual([]);
});
it("marks Pi's argument-validation and unknown-tool errors as malformed calls", () => {
  const rounds = roundResults(undefined, [
    result("v", 'Validation failed for tool "edit":\n- path: required', true),
    result("u", 'Tool "grepx" not found', true),
    result("f", "Command exited with code 1", true),
  ]);
  expect(rounds.map((round) => round.malformed)).toStrictEqual([true, true, false]);
});
it("reads a single tool result's error kind for the change-approach note", () => {
  expect(
    toolResultEvidence({ content: [{ text: 'Tool "x" not found', type: "text" }], isError: true }),
  ).toStrictEqual({ isError: true, malformed: true });
  expect(
    toolResultEvidence({ content: [{ text: "boom", type: "text" }], isError: true }),
  ).toStrictEqual({ isError: true, malformed: false });
});
