// This file runs with bun.
import { expect, it } from "vitest";

import {
  isRecord,
  nonEmptyString,
  parseJson,
  parseJsonOrRaw,
  stringifyJson,
  tryParseJson,
} from "../src/json.ts";

it("parses, serializes, and guards JSON values via JSON.parse", () => {
  expect(
    parseJson(
      '{"ok":true,"neg":-7,"big":18446744073709551615,"flt":1.5,"nil":null,"arr":[1,"a",false]}',
    ),
  ).toStrictEqual({
    ok: true,
    neg: -7,
    big: 18446744073709552000,
    flt: 1.5,
    nil: null,
    arr: [1, "a", false],
  });
  expect(() => parseJson("not json")).toThrow();
  expect(tryParseJson('{"a":1}')).toStrictEqual({ a: 1 });
  expect(tryParseJson("not json")).toBeUndefined();
  expect(parseJsonOrRaw("null")).toBeNull();
  expect(parseJsonOrRaw("not json")).toStrictEqual({ raw: "not json" });
  expect(stringifyJson({ a: 1 })).toBe('{"a":1}');
  expect(isRecord({ a: 1 })).toBe(true);
  expect(isRecord([])).toBe(false);
  expect(nonEmptyString("ok")).toBe("ok");
  expect(nonEmptyString("")).toBeUndefined();
});
