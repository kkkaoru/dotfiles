// This file runs with Bun.
import { expect, test } from "vitest";
import { isJsonObject, isJsonValue } from "./json-value.ts";

test("accepts only finite JSON data for tool inputs", () => {
  expect(isJsonObject({ nested: [null, true, "text", 1, { ok: false }] })).toBe(true);
  expect(isJsonObject(Object.create(null))).toBe(true);
  expect(isJsonObject([])).toBe(false);
  expect(isJsonObject(new Date())).toBe(false);
  expect(isJsonObject({ bad: undefined })).toBe(false);
  expect(isJsonValue(Number.POSITIVE_INFINITY)).toBe(false);
  expect(isJsonValue(Symbol("bad"))).toBe(false);
  expect(isJsonValue(() => 1)).toBe(false);
});
