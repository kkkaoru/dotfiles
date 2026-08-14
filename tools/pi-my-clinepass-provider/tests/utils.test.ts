import { describe, expect, it } from "vitest";
import { isRecord, stringValue } from "../src/utils.ts";

describe("I/O boundary guards", () => {
  it("recognizes plain records", () => {
    expect(isRecord({ key: "value" })).toBe(true);
    expect(isRecord(null)).toBe(false);
    expect(isRecord([])).toBe(false);
    expect(isRecord("value")).toBe(false);
  });

  it("extracts only strings", () => {
    expect(stringValue("value")).toBe("value");
    expect(stringValue(1)).toBeUndefined();
  });
});
