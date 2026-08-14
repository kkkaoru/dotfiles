import { describe, expect, it } from "vitest";
import {
  CLINEPASS_OPENAI_COMPAT,
  DEFAULT_THINKING_LEVEL_MAP,
  MODELS,
  modelIds,
  modelsFromClineCatalog,
} from "../src/models.ts";

function modelById(modelId: string) {
  const model = MODELS.find((candidate) => candidate.id === modelId);
  if (model === undefined) {
    throw new Error(`Missing test model: ${modelId}`);
  }
  return model;
}

describe("ClinePass model metadata", () => {
  it("contains metadata for current Cline CLI models", () => {
    expect(modelIds().includes("cline-pass/glm-5.3")).toBe(true);
    expect(modelIds().includes("cline-pass/qwen3.8-max")).toBe(true);
  });

  it("uses metadata verified from the Cline catalog", () => {
    expect(modelById("cline-pass/glm-5.3")).toStrictEqual({
      compat: CLINEPASS_OPENAI_COMPAT,
      contextWindow: 128_000,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      id: "cline-pass/glm-5.3",
      input: ["text"],
      maxTokens: 8192,
      name: "GLM-5.3 (ClinePass)",
      reasoning: true,
      thinkingLevelMap: {
        off: null,
        minimal: null,
        low: null,
        medium: null,
        high: null,
        xhigh: null,
      },
    });
    expect(modelById("cline-pass/qwen3.8-max").input).toStrictEqual(["text", "image"]);
  });
});

describe("Cline CLI catalog mapping", () => {
  it("registers only models returned by the Cline CLI and preserves its order", () => {
    const models = modelsFromClineCatalog([
      { modelId: "cline-pass/qwen3.8-max", name: "Qwen3.8 Max" },
      { modelId: "cline-pass/glm-5.3", name: "cline-pass/glm-5.3" },
    ]);
    expect(models.map((model) => model.id)).toStrictEqual([
      "cline-pass/qwen3.8-max",
      "cline-pass/glm-5.3",
    ]);
    expect(models.map((model) => model.name)).toStrictEqual([
      "Qwen3.8 Max (ClinePass)",
      "GLM-5.3 (ClinePass)",
    ]);
  });

  it("deduplicates CLI entries using the last entry", () => {
    const models = modelsFromClineCatalog([
      { modelId: "cline-pass/glm-5.3", name: "Old Name" },
      { modelId: "cline-pass/glm-5.3", name: "cline-pass/glm-5.3" },
    ]);
    expect(models.map((model) => model.name)).toStrictEqual(["GLM-5.3 (ClinePass)"]);
  });

  it("provides conservative metadata for a newly discovered model", () => {
    const models = modelsFromClineCatalog([
      { modelId: "cline-pass/future-model", name: "Future Model" },
      { modelId: "cline-pass/raw-model", name: "cline-pass/raw-model" },
    ]);
    expect(models).toStrictEqual([
      {
        compat: CLINEPASS_OPENAI_COMPAT,
        contextWindow: 128_000,
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
        id: "cline-pass/future-model",
        input: ["text"],
        maxTokens: 8192,
        name: "Future Model (ClinePass)",
        reasoning: true,
        thinkingLevelMap: DEFAULT_THINKING_LEVEL_MAP,
      },
      {
        compat: CLINEPASS_OPENAI_COMPAT,
        contextWindow: 128_000,
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
        id: "cline-pass/raw-model",
        input: ["text"],
        maxTokens: 8192,
        name: "cline-pass/raw-model (ClinePass)",
        reasoning: true,
        thinkingLevelMap: DEFAULT_THINKING_LEVEL_MAP,
      },
    ]);
  });
});
