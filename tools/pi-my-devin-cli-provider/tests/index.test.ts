// This file runs with Bun.
import type { ProviderModelConfig } from "@earendil-works/pi-coding-agent";
import { expect, test, vi } from "vitest";
import devinExtension, { registerDevinProvider } from "../index.ts";

test("registers the Devin CLI ACP provider without API credentials", () => {
  const registerProvider = vi.fn();

  registerDevinProvider({ registerProvider });

  expect(registerProvider).toHaveBeenCalledTimes(1);
  const call = registerProvider.mock.calls[0];
  expect(call?.[0]).toBe("devin");
  expect(call?.[1]).toMatchObject({
    name: "Devin CLI",
    baseUrl: "https://app.devin.ai",
    apiKey: "devin-cli-managed",
    api: "devin-cli-acp",
  });
  expect(call?.[1]?.models?.map((model: ProviderModelConfig) => model.id)).toStrictEqual([
    "adaptive",
    "swe-1-7",
    "swe-1-7-medium",
    "claude-sonnet-5-medium",
    "claude-opus-5-medium",
    "gpt-5-6-luna-medium",
    "gemini-3-7-flash-medium",
    "glm-5-2",
    "kimi-k3-high",
  ]);
  expect(typeof call?.[1]?.refreshModels).toBe("function");
  expect(typeof call?.[1]?.streamSimple).toBe("function");
});

test("registers the provider and compaction hooks from the default export", () => {
  const registerProvider = vi.fn();
  const on = vi.fn();

  Reflect.apply(devinExtension, undefined, [{ registerProvider, on }]);

  expect(registerProvider).toHaveBeenCalledTimes(1);
  expect(registerProvider.mock.calls[0]?.[0]).toBe("devin");
  expect(on).toHaveBeenCalledWith("session_before_compact", expect.any(Function));
  expect(on).toHaveBeenCalledWith("session_compact", expect.any(Function));
});
