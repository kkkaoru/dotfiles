// This TypeScript file is executed with Bun.
import { expect, test } from "vitest";
import { textAt, translateFast } from "./fast.ts";

test("a missing slot keeps the fallback text", () => {
  expect(textAt(["first"], 0, "body")).toBe("first");
  expect(textAt([], 0, "body")).toBe("body");
});

test("four lines are two requests", async () => {
  const calls = { count: 0 };
  const translated = await translateFast("a1\na2\nb1\nb2", async (text) => {
    calls.count += 1;
    return text.replaceAll("a", "A").replaceAll("b", "B");
  });
  expect(calls.count).toBe(2);
  expect(translated).toBe("A1\nA2\nB1\nB2");
});

test("an unchanged pair is translated line by line", async () => {
  const calls = { count: 0 };
  const translated = await translateFast("aa\nbb", async (text) => {
    calls.count += 1;
    return text.includes("\n") ? text : `ja:${text}`;
  });
  expect(calls.count).toBe(3);
  expect(translated).toBe("ja:aa\nja:bb");
});

test("several lines are one request and keep their markers", async () => {
  const calls = { count: 0 };
  const translated = await translateFast("- 一行目\n\n```\ncode\n```\n2. 二行目", async (text) => {
    calls.count += 1;
    return text.replaceAll("一行目", "first").replaceAll("二行目", "second");
  });
  expect(calls.count).toBe(1);
  expect(translated).toBe("- first\n\n```\ncode\n```\n2. second");
});

test("a reply that merges lines falls back to one request per line", async () => {
  const calls = { count: 0 };
  const translated = await translateFast("一行目\n二行目", async (text) => {
    calls.count += 1;
    return text.includes("\n") ? "merged" : text;
  });
  expect(calls.count).toBe(3);
  expect(translated).toBe("一行目\n二行目");
});

test("a fenced block alone is not sent", async () => {
  const calls = { count: 0 };
  const translated = await translateFast("```\ncode\n```", async () => {
    calls.count += 1;
    return "nope";
  });
  expect(calls.count).toBe(0);
  expect(translated).toBe("```\ncode\n```");
});

test("a table is not sent as one block", async () => {
  const calls = { count: 0 };
  await translateFast("| a | b |\n| --- | --- |\n| 猫 | 犬 |", async (text) => {
    calls.count += 1;
    return text;
  });
  expect(calls.count).toBe(4);
});
