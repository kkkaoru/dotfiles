// This TypeScript file is executed with Bun.
import { expect, it } from "vitest";
import { eventToolCallId, toolResultFailed } from "./tool-event.ts";

it("reads only typed tool identities and explicit failure", () => {
  expect(eventToolCallId(null)).toBeUndefined();
  expect(eventToolCallId({})).toBeUndefined();
  expect(eventToolCallId({ toolCallId: 42 })).toBeUndefined();
  expect(eventToolCallId({ toolCallId: "call-1" })).toBe("call-1");
  expect(toolResultFailed(null)).toBe(false);
  expect(toolResultFailed({})).toBe(false);
  expect(toolResultFailed({ isError: false })).toBe(false);
  expect(toolResultFailed({ isError: true })).toBe(true);
});
