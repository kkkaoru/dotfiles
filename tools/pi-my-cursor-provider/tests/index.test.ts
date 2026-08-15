import type { ProviderModelConfig } from "@earendil-works/pi-coding-agent";
import { expect, test, vi } from "vitest";
import cursorExtension from "../index.ts";

test("registers the compatible cursor/auto provider", () => {
  const registerProvider = vi.fn();
  const on = vi.fn();

  cursorExtension({ registerProvider, on } as never);

  expect(registerProvider).toHaveBeenCalledTimes(1);
  const call = registerProvider.mock.calls[0];
  expect(call?.[0]).toBe("cursor");
  expect(call?.[1]).toMatchObject({
    name: "Cursor",
    apiKey: "$CURSOR_API_KEY",
    api: "cursor-agent",
  });
  expect(call?.[1]?.models?.map((model: ProviderModelConfig) => model.id)).toStrictEqual([
    "auto",
    "composer-2.5",
    "claude-sonnet-4-6",
    "claude-opus-5",
    "gpt-5.6-sol",
    "gpt-5.4",
    "gemini-3.1-pro",
    "grok-4.6",
    "kimi-k3",
    "glm-5.2",
  ]);
  expect(typeof call?.[1]?.refreshModels).toBe("function");
  expect(typeof call?.[1]?.streamSimple).toBe("function");
});

test("registers the compaction hook and provider together from the default export", () => {
  const registerProvider = vi.fn();
  const on = vi.fn();

  cursorExtension({ registerProvider, on } as never);

  expect(registerProvider).toHaveBeenCalledTimes(1);
  expect(on).toHaveBeenCalledWith("session_before_compact", expect.any(Function));
});
