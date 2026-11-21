// This TypeScript file is executed with Bun.
import { expect, test } from "vitest";
import { changedPart, fingerprint } from "./fingerprint.ts";

const base = {
  messages: [
    { role: "system", content: "be brief" },
    { role: "user", content: "one" },
    { role: "assistant", content: "two" },
  ],
  tools: [{ name: "bash" }],
};

test("a turn that only appends a message keeps the prefix", () => {
  const previous = fingerprint(base);
  const next = fingerprint({
    ...base,
    messages: [...base.messages, { role: "user", content: "3" }],
  });
  expect(changedPart(previous, next)).toBeNull();
});

test("each part is named, earliest first", () => {
  const previous = fingerprint(base);
  const system = fingerprint({
    ...base,
    messages: [{ role: "system", content: "x" }, ...base.messages.slice(1)],
  });
  expect(changedPart(previous, system)).toBe("system");

  const tools = fingerprint({ ...base, tools: [{ name: "edit" }] });
  expect(changedPart(previous, tools)).toBe("tools");

  const edited = [base.messages[0], { role: "user", content: "z" }, ...base.messages.slice(2)];
  expect(changedPart(previous, fingerprint({ ...base, messages: edited }))).toBe("messages");

  const everything = fingerprint({ system: "s", tools: [], messages: [] });
  expect(changedPart(previous, everything)).toBe("system");
});

test("a model switch is a break even when the prompt is unchanged", () => {
  const previous = fingerprint({ ...base, model: "a" });
  expect(changedPart(previous, fingerprint({ ...base, model: "b" }))).toBe("model");
  expect(changedPart(previous, fingerprint({ ...base, model: "a" }))).toBeNull();
});

test("a shorter history is a break", () => {
  const previous = fingerprint(base);
  const shorter = fingerprint({ ...base, messages: base.messages.slice(0, 1) });
  expect(changedPart(previous, shorter)).toBe("messages");
});

test("a top-level system prompt is read where the provider puts it", () => {
  const previous = fingerprint({ system: "a", messages: [] });
  expect(changedPart(previous, fingerprint({ system: "b", messages: [] }))).toBe("system");
});

test("a payload that is not an object is still fingerprinted", () => {
  const empty = fingerprint(undefined);
  expect(empty).toStrictEqual(fingerprint({}));
  expect(changedPart(empty, fingerprint({ messages: "not a list" }))).toBeNull();
});
