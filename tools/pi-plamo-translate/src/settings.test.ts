// This TypeScript file is executed with Bun.
import { expect, test } from "vitest";
import { missingFile, readPlamoSettings } from "./settings.ts";

test("missing settings use Japanese display and English prompts", () => {
  const error = Object.assign(new Error("missing"), { code: "ENOENT" });
  expect(
    readPlamoSettings(() => {
      throw error;
    }),
  ).toStrictEqual({ replyLanguage: "ja", promptLanguage: "en", port: 18555 });
});

test("a settings object without the section uses the defaults", () => {
  expect(readPlamoSettings(() => '{"other":1}')).toStrictEqual({
    replyLanguage: "ja",
    promptLanguage: "en",
    port: 18555,
  });
});

test("an array or null settings file uses the defaults", () => {
  expect(readPlamoSettings(() => "[]")).toStrictEqual({
    replyLanguage: "ja",
    promptLanguage: "en",
    port: 18555,
  });
  expect(readPlamoSettings(() => "null")).toStrictEqual({
    replyLanguage: "ja",
    promptLanguage: "en",
    port: 18555,
  });
});

test("a non-object section uses the defaults", () => {
  expect(readPlamoSettings(() => '{"pi-plamo-translate":true}')).toStrictEqual({
    replyLanguage: "ja",
    promptLanguage: "en",
    port: 18555,
  });
});

test("configured languages and port are kept", () => {
  expect(
    readPlamoSettings(
      () =>
        '{"pi-plamo-translate":{"replyLanguage":"zh-hans","promptLanguage":"off","port":18556}}',
    ),
  ).toStrictEqual({ replyLanguage: "zh-hans", promptLanguage: "off", port: 18556 });
});

test("blank languages and a bad port fall back", () => {
  expect(
    readPlamoSettings(
      () => '{"pi-plamo-translate":{"replyLanguage":"  ","promptLanguage":1,"port":0}}',
    ),
  ).toStrictEqual({ replyLanguage: "ja", promptLanguage: "en", port: 18555 });
});

test("a port above 65535 falls back", () => {
  expect(readPlamoSettings(() => '{"pi-plamo-translate":{"port":70000}}')).toStrictEqual({
    replyLanguage: "ja",
    promptLanguage: "en",
    port: 18555,
  });
});

test("invalid JSON is reported", () => {
  expect(() => readPlamoSettings(() => "{")).toThrow("plamo-translate: reading settings failed");
});

test("errors other than a missing file propagate", () => {
  expect(() =>
    readPlamoSettings(() => {
      throw new Error("disk");
    }),
  ).toThrow("plamo-translate: reading settings failed");
});

test("a non-object failure is not a missing file", () => {
  expect(missingFile(1)).toBe(false);
  expect(missingFile({ code: "ENOENT" })).toBe(true);
});
