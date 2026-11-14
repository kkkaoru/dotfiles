// This TypeScript file is executed with Bun.
import { expect, test } from "vitest";
import {
  assistantTexts,
  isCommand,
  needsDisplayTranslation,
  remember,
  replyRule,
  shouldTranslatePrompt,
  statusText,
  textBlock,
  transformedInput,
} from "./policy.ts";

test("status counts the finished requests", () => {
  expect(statusText(1, 4)).toBe("PLaMo: 翻訳中… 1/4");
});

test("slash commands and bang commands are left alone", () => {
  expect(isCommand("  /goal 調べる")).toBe(true);
  expect(isCommand("!ls")).toBe(true);
  expect(isCommand("調べる")).toBe(false);
});

test("a Japanese prompt is translated and an extension prompt is not", () => {
  expect(shouldTranslatePrompt("タブを追加して", "interactive", "en")).toBe(true);
  expect(shouldTranslatePrompt("add a tab", "interactive", "en")).toBe(false);
  expect(shouldTranslatePrompt("タブを追加して", "extension", "en")).toBe(false);
  expect(shouldTranslatePrompt("/goal 調べる", "interactive", "en")).toBe(false);
  expect(shouldTranslatePrompt("タブを追加して", "rpc", "off")).toBe(false);
});

test("both directions off installs no rule", () => {
  expect(replyRule("off", "off")).toBeUndefined();
});

test("prompt translation alone asks the model to stay in English", () => {
  expect(replyRule("off", "en")).toBe(
    "The user writes in Japanese. Their messages are machine-translated into English before you read them. Read and write English.",
  );
});

test("reply translation tells the model what the person sees", () => {
  expect(replyRule("ja", "en")).toBe(
    "The user writes in Japanese. Their messages are machine-translated into English before you read them, and your replies are machine-translated into `ja` for display only. Read and write English. Only inline code and fenced code blocks are shown exactly as written. Put anything the user must read or copy verbatim in one of them: commands, file paths, identifiers, option and flag names, literal values, error messages and quoted UI text. Do not wrap ordinary prose.",
  );
});

test("streaming and Japanese text are not sent for display translation", () => {
  expect(needsDisplayTranslation("Add a tab", "ja", false)).toBe(true);
  expect(needsDisplayTranslation("タブを追加する", "ja", false)).toBe(false);
  expect(needsDisplayTranslation("Add a tab", "ja", true)).toBe(false);
  expect(needsDisplayTranslation("Add a tab", "off", false)).toBe(false);
});

test("a prompt without images transforms the text only", () => {
  expect(transformedInput("Add a tab", undefined)).toStrictEqual({
    action: "transform",
    text: "Add a tab",
  });
});

test("attached images are kept with the translated prompt", () => {
  expect(
    transformedInput("Add a tab", [{ type: "image", data: "abc", mimeType: "image/png" }]),
  ).toStrictEqual({
    action: "transform",
    text: "Add a tab",
    images: [{ type: "image", data: "abc", mimeType: "image/png" }],
  });
});

test("only non-empty assistant text blocks are collected", () => {
  expect(textBlock({ type: "thinking", thinking: "hmm" })).toBeUndefined();
  expect(textBlock({ type: "text", text: "  " })).toBeUndefined();
  expect(textBlock({ type: "text", text: " Add a tab " })).toBe("Add a tab");
  expect(textBlock("nope")).toBeUndefined();
  expect(assistantTexts({ role: "user", content: [{ type: "text", text: "hi" }] })).toStrictEqual(
    [],
  );
  expect(assistantTexts({ role: "assistant" })).toStrictEqual([]);
  expect(
    assistantTexts({
      role: "assistant",
      content: [{ type: "text", text: " Add a tab " }, { type: "toolCall" }],
    }),
  ).toStrictEqual(["Add a tab"]);
});

test("the cache keeps the newest 200 entries", () => {
  const map = new Map<string, string>();
  remember(map, "first", "1");
  remember(map, "first", "1b");
  expect(map.get("first")).toBe("1b");
  Array.from({ length: 200 }, (_unused, index) => String(index)).forEach((key) => {
    remember(map, key, "x");
  });
  expect(map.has("first")).toBe(false);
  expect(map.size).toBe(200);
});
