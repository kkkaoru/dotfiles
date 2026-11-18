// This TypeScript file is executed with Bun.
import { expect, test } from "vitest";
import { capImages, OMITTED } from "./budget.ts";

const image = (name: string): { readonly name: string; readonly type: string } => ({
  name,
  type: "image",
});
const text = (value: string): { readonly text: string; readonly type: string } => ({
  text: value,
  type: "text",
});

function conversation(names: readonly string[]): unknown[] {
  return [
    { content: "plain prompt", role: "user" },
    { content: [text("first read"), image(names[0] ?? "")], role: "toolResult" },
    { content: [image(names[1] ?? ""), image(names[2] ?? "")], role: "user" },
    { content: [text("no image here")], role: "toolResult" },
  ];
}

test("the newest images stay and older ones become a note", () => {
  const messages = conversation(["a", "b", "c"]);
  expect(capImages(messages, 1, 99)).toBe(2);
  expect(messages).toStrictEqual([
    { content: "plain prompt", role: "user" },
    { content: [text("first read"), { text: OMITTED, type: "text" }], role: "toolResult" },
    { content: [{ text: OMITTED, type: "text" }, image("c")], role: "user" },
    { content: [text("no image here")], role: "toolResult" },
  ]);
});

test("a conversation under the cap is left alone", () => {
  const messages = conversation(["a", "b", "c"]);
  expect(capImages(messages, 3, 99)).toBe(0);
  expect(messages).toStrictEqual(conversation(["a", "b", "c"]));
});

test("a per-message cap trims inside one message even when the request is small", () => {
  const messages = conversation(["a", "b", "c"]);
  expect(capImages(messages, 99, 1)).toBe(1);
  expect(messages).toStrictEqual([
    { content: "plain prompt", role: "user" },
    { content: [text("first read"), image("a")], role: "toolResult" },
    { content: [{ text: OMITTED, type: "text" }, image("c")], role: "user" },
    { content: [text("no image here")], role: "toolResult" },
  ]);
});

test("a cap of zero drops every image, and other content is ignored", () => {
  const messages = conversation(["a", "b", "c"]);
  expect(capImages(messages, 0, 99)).toBe(3);
  expect(JSON.stringify(messages)).not.toContain('"type":"image"');
  expect(capImages([{ content: "text only" }, { role: "assistant" }, null, 3], 2, 2)).toBe(0);
});
