// This file runs with Bun.
import { expect, it } from "vitest";
import {
  normalizeContext,
  type Api,
  type AssistantMessageEvent,
  type Model,
} from "@earendil-works/pi-ai";
import type { ExtensionAPI, ProviderConfig } from "@earendil-works/pi-coding-agent";
import claudeSdkExtension, { CLAUDE_SDK_PROVIDER_ID } from "../index.ts";

const model: Model<Api> = {
  id: "claude-haiku-5-5",
  name: "Claude Haiku 5.5",
  api: "claude-agent-sdk",
  provider: "claude-sdk",
  baseUrl: "https://api.anthropic.com",
  reasoning: true,
  input: ["text"],
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
  contextWindow: 1_000_000,
  maxTokens: 128_000,
};

async function collect(
  stream: AsyncIterable<AssistantMessageEvent>,
): Promise<AssistantMessageEvent[]> {
  const events: AssistantMessageEvent[] = [];
  for await (const event of stream) {
    events.push(event);
  }
  return events;
}

it("registers the provider with verified plan models and a lazy stream", async () => {
  const registered: { name: unknown; config: ProviderConfig | undefined }[] = [];
  const pi: Pick<ExtensionAPI, "registerProvider"> = {
    registerProvider: (name: unknown, config?: ProviderConfig): void => {
      registered.push({ name, config });
    },
  };
  claudeSdkExtension(pi);
  const [{ name, config } = { name: undefined, config: undefined }] = registered;
  expect(name).toBe(CLAUDE_SDK_PROVIDER_ID);
  expect(config?.api).toBe("claude-agent-sdk");
  expect(
    config?.models?.map((item) => ({
      id: item.id,
      contextWindow: "contextWindow" in item ? item.contextWindow : undefined,
    })),
  ).toStrictEqual([
    { id: "claude-opus-5-5", contextWindow: 1_000_000 },
    { id: "claude-fable-5-1", contextWindow: 1_000_000 },
    { id: "claude-sonnet-5-5", contextWindow: 1_000_000 },
    { id: "claude-haiku-5-5", contextWindow: 1_000_000 },
    { id: "claude-opus-5", contextWindow: 1_000_000 },
    { id: "claude-fable-5", contextWindow: 1_000_000 },
    { id: "claude-sonnet-5", contextWindow: 1_000_000 },
    { id: "claude-opus-4-8", contextWindow: 1_000_000 },
    { id: "claude-opus-4-7", contextWindow: 1_000_000 },
    { id: "claude-opus-4-6", contextWindow: 1_000_000 },
    { id: "claude-sonnet-4-6", contextWindow: 1_000_000 },
    { id: "claude-haiku-4-5-20251001", contextWindow: 200_000 },
  ]);
  const controller = new AbortController();
  controller.abort();
  const stream = config?.streamSimple?.(model, normalizeContext({ messages: [] }), {
    signal: controller.signal,
  });
  const events = stream === undefined ? [] : await collect(stream);
  expect(events.at(-1)?.type).toBe("error");
});
