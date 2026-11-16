// This file runs with bun.
import { expect, it } from "vitest";

import {
  levelMap,
  mergeDiscovered,
  runtimeModelId,
  staticModels,
  thinkingBudget,
} from "../src/models.ts";

it("maps a public id and pi thinking level to the CLI runtime id", () => {
  expect(runtimeModelId("gemini-4-hydrogen", "medium")).toBe("gemini-4-hydrogen-medium");
  expect(runtimeModelId("gemini-4-hydrogen", "xhigh")).toBe("gemini-4-hydrogen-xhigh");
  expect(runtimeModelId("gemini-3.8-flash", "low")).toBe("gemini-3.8-flash-low");
  expect(runtimeModelId("claude-sonnet-5-5", "high")).toBe("claude-sonnet-5-5-high");
  expect(runtimeModelId("gpt-oss-120b", undefined)).toBe("gpt-oss-120b-medium");
  expect(runtimeModelId("gemini-pro-orchestrator", "high")).toBe("gemini-pro-orchestrator-g1-pro");
  expect(runtimeModelId("claude-opus-5-5", "xhigh")).toBe("claude-opus-5-5-high");
  expect(runtimeModelId("gemini-9-unknown", "medium")).toBe("gemini-9-unknown");
});

it("grows thinkingBudget with the requested level", () => {
  expect(thinkingBudget("minimal")).toBe(1024);
  expect(thinkingBudget("low")).toBe(4096);
  expect(thinkingBudget("medium")).toBe(8192);
  expect(thinkingBudget("high")).toBe(16384);
  expect(thinkingBudget("xhigh")).toBe(24576);
  expect(thinkingBudget("max")).toBe(24576);
  expect(thinkingBudget(undefined)).toBe(4096);
});

it("exposes static catalog and merges newly discovered families", () => {
  const staticIds = staticModels().map((model) => model.id);
  expect(staticIds).toStrictEqual([
    "gemini-4-hydrogen",
    "gemini-4-argon",
    "gemini-3.8-flash",
    "claude-opus-5-5",
    "claude-sonnet-5-5",
    "gpt-oss-120b",
    "gemini-pro-orchestrator",
  ]);
  expect(levelMap.fourTier.xhigh).toBe("xhigh");
  expect(levelMap.mediumOnly.low).toBeNull();
  expect(mergeDiscovered([]).length).toBe(7);

  const mergedIds = mergeDiscovered([
    { id: "gemini-4-hydrogen-medium" },
    { id: "gemini-5-nebula-high", displayName: "Gemini 5 Nebula (High)" },
    { id: "gemini-5-nebula-low" },
    { id: "gemini-6-custom" },
  ]).map((model) => model.id);
  expect(mergedIds).toStrictEqual([
    "gemini-4-hydrogen",
    "gemini-4-argon",
    "gemini-3.8-flash",
    "claude-opus-5-5",
    "claude-sonnet-5-5",
    "gpt-oss-120b",
    "gemini-pro-orchestrator",
    "gemini-5-nebula",
    "gemini-6-custom",
  ]);
});
