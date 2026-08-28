// This TypeScript file is executed with Bun.
import { expect, it } from "vitest";
import { fastEligible } from "./fast.ts";

it("accepts GPT-5 models only on priority-capable providers", () => {
  expect(fastEligible({ id: "gpt-5.6", provider: "openai-codex" })).toBe(true);
  expect(fastEligible({ id: "gpt-5.6", provider: "other" })).toBe(false);
  expect(fastEligible({ id: "gpt-4.1", provider: "openai" })).toBe(false);
  expect(fastEligible(undefined)).toBe(false);
});
