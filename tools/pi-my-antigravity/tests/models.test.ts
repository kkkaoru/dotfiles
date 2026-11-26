import { describe, expect, it } from "vitest";

import { DEFAULT_MODEL } from "../src/constants.ts";
import { mergeDiscovered, modelConfig, parseModelList, staticModels } from "../src/models.ts";

describe("modelConfig", () => {
  it("describes a free, reasoning, text-and-image model", () => {
    expect(modelConfig("gemini-4-hydrogen")).toMatchObject({
      id: "gemini-4-hydrogen",
      name: "gemini-4-hydrogen",
      reasoning: true,
      input: ["text", "image"],
      cost: { cacheRead: 0, cacheWrite: 0, input: 0, output: 0 },
      contextWindow: expect.any(Number),
      maxTokens: expect.any(Number),
    });
  });
});

describe("staticModels", () => {
  it("offers the CLI default model", () => {
    expect(staticModels().map((model) => model.id)).toEqual([DEFAULT_MODEL]);
  });
});

describe("mergeDiscovered", () => {
  it("keeps the static model first and adds new ids once", () => {
    const merged = mergeDiscovered(["gemini-extra", "gemini-extra", DEFAULT_MODEL, ""]);
    expect(merged.map((model) => model.id)).toEqual([DEFAULT_MODEL, "gemini-extra"]);
  });
});

describe("parseModelList", () => {
  it("reads a plain array of ids", () => {
    expect(parseModelList(["gemini-a", "gemini-b"])).toEqual(["gemini-a", "gemini-b"]);
  });

  it("reads a models array and strips the models/ prefix", () => {
    expect(parseModelList({ models: [{ name: "models/gemini-a" }, { id: "gemini-b" }] })).toEqual([
      "gemini-a",
      "gemini-b",
    ]);
  });

  it("reads a data array", () => {
    expect(parseModelList({ data: [{ model: "gemini-c" }] })).toEqual(["gemini-c"]);
  });

  it("drops entries it cannot identify", () => {
    expect(parseModelList({ items: [{}, { id: 3 }, { id: "gemini-d" }] })).toEqual(["gemini-d"]);
  });

  it("returns nothing for an unknown shape", () => {
    expect(parseModelList({ nope: true })).toEqual([]);
    expect(parseModelList("gemini-a")).toEqual([]);
  });
});

describe("parseModelList entries", () => {
  it("drops a non-string, non-record entry", () => {
    expect(parseModelList([3, "gemini-e"])).toEqual(["gemini-e"]);
  });
});
