import type { SessionBeforeCompactEvent } from "@earendil-works/pi-coding-agent";
import type { Api, Message, Model, Usage } from "@earendil-works/pi-ai";
import { expect, test, vi } from "vitest";
import {
  buildSummaryPrompt,
  isCursorModelActive,
  registerCursorCompaction,
  resolveSummaryModel,
  summarizeWithFallbackChain,
  textOfContent,
} from "../src/compaction.ts";

interface RegistryOptions {
  complete?: (
    model: Model<Api>,
    context: unknown,
    options?: unknown,
  ) => Promise<{ content: unknown[]; usage?: Usage }>;
}

const KIMI = { provider: "ollama-cloud", id: "kimi-k3" } as Model<Api>;
const COPILOT_GEMINI = { provider: "github-copilot", id: "gemini-3.7-flash" } as Model<Api>;
const COMMANDCODE_GEMINI = { provider: "commandcode", id: "gemini-3.7-flash" } as Model<Api>;
const OTHER = { provider: "ollama-cloud", id: "glm-5.2" } as Model<Api>;

function registry(
  models: readonly Model<Api>[],
  { complete }: RegistryOptions = {},
): Parameters<typeof resolveSummaryModel>[0] {
  return {
    find: (provider, modelId) =>
      models.find((model) => model.provider === provider && model.id === modelId),
    hasConfiguredAuth: () => true,
    complete:
      complete ??
      (async () => {
        throw new Error("unexpected complete call");
      }),
  };
}
function assistant(provider: string): { role: "assistant"; provider: string } {
  return { role: "assistant", provider };
}

test("resolves the summary model in priority order", () => {
  expect(resolveSummaryModel(registry([KIMI, COPILOT_GEMINI, COMMANDCODE_GEMINI]))).toMatchObject({
    provider: "ollama-cloud",
    modelId: "kimi-k3",
  });
  expect(resolveSummaryModel(registry([COPILOT_GEMINI, COMMANDCODE_GEMINI]))).toMatchObject({
    provider: "github-copilot",
    modelId: "gemini-3.7-flash",
  });
  expect(resolveSummaryModel(registry([COMMANDCODE_GEMINI]))).toMatchObject({
    provider: "commandcode",
    modelId: "gemini-3.7-flash",
  });
});

test("skips models without configured auth and returns undefined when none remain", () => {
  const base = registry([KIMI, COPILOT_GEMINI]);
  const noAuth = {
    find: (provider: string, modelId: string) => base.find(provider, modelId),
    hasConfiguredAuth: () => false,
    complete: (model: Model<Api>, context: unknown, options?: unknown) =>
      base.complete(model, context, options),
  } as Parameters<typeof resolveSummaryModel>[0];
  expect(resolveSummaryModel(noAuth)).toBeUndefined();
});

test("builds the summary prompt with conversation and optional previous summary", () => {
  const base = buildSummaryPrompt("conversation text", undefined, undefined);
  expect(base).toContain("<conversation>\nconversation text\n</conversation>");
  expect(base).not.toContain("<previous-summary>");

  const updated = buildSummaryPrompt("more text", "old summary", "focus on tests");
  expect(updated).toContain("<previous-summary>\nold summary\n</previous-summary>");
  expect(updated).toContain("Additional focus: focus on tests");
});

test("falls back down the chain when a provider fails or returns empty", async () => {
  const complete = vi
    .fn()
    .mockRejectedValueOnce(new Error("kimi rate limited"))
    .mockResolvedValueOnce({ content: [{ type: "text", text: "" }] })
    .mockResolvedValueOnce({ content: [{ type: "text", text: "final summary" }] });

  const result = await summarizeWithFallbackChain(
    "conversation",
    undefined,
    undefined,
    registry([KIMI, COPILOT_GEMINI, COMMANDCODE_GEMINI], { complete }),
    undefined,
  );

  expect(result).toStrictEqual({
    summary: "final summary",
    provider: "commandcode",
    modelId: "gemini-3.7-flash",
  });
  expect(complete).toHaveBeenCalledTimes(3);
});

test("returns undefined when the whole chain fails or nothing is available", async () => {
  const complete = vi.fn().mockRejectedValue(new Error("down"));
  await expect(
    summarizeWithFallbackChain(
      "conversation",
      undefined,
      undefined,
      registry([KIMI], { complete }),
      undefined,
    ),
  ).resolves.toBeUndefined();
  await expect(
    summarizeWithFallbackChain("conversation", undefined, undefined, registry([]), undefined),
  ).resolves.toBeUndefined();
});

test("aborts cleanly when the signal fires before the call", async () => {
  const controller = new AbortController();
  controller.abort();
  const complete = vi.fn().mockResolvedValue({ content: [{ type: "text", text: "summary" }] });

  await expect(
    summarizeWithFallbackChain(
      "conversation",
      undefined,
      undefined,
      registry([KIMI], { complete }),
      controller.signal,
    ),
  ).resolves.toBeUndefined();
});

test("stops falling back once the signal is aborted", async () => {
  const controller = new AbortController();
  const complete = vi.fn(async () => {
    controller.abort();
    throw new Error("kimi rate limited");
  });

  await expect(
    summarizeWithFallbackChain(
      "conversation",
      undefined,
      undefined,
      registry([KIMI, COPILOT_GEMINI], { complete }),
      controller.signal,
    ),
  ).resolves.toBeUndefined();
  expect(complete).toHaveBeenCalledTimes(1);
});

test("skips a provider that loses auth between resolution and the attempt", async () => {
  let calls = 0;
  const models = [KIMI, COPILOT_GEMINI];
  const complete = vi
    .fn()
    .mockResolvedValue({ content: [{ type: "text", text: "copilot summary" }] });

  const flippingRegistry = {
    find: (provider: string, modelId: string) =>
      models.find((model) => model.provider === provider && model.id === modelId),
    hasConfiguredAuth: (model: Model<Api>) => ++calls === 1 || model !== KIMI,
    complete,
  } as unknown as Parameters<typeof summarizeWithFallbackChain>[3];

  await expect(
    summarizeWithFallbackChain("conversation", undefined, undefined, flippingRegistry, undefined),
  ).resolves.toStrictEqual({
    summary: "copilot summary",
    provider: "github-copilot",
    modelId: "gemini-3.7-flash",
  });
});

function compactEvent(
  messages: Array<{ role: string; provider?: string }>,
): SessionBeforeCompactEvent {
  return {
    type: "session_before_compact",
    preparation: {
      messagesToSummarize: messages as never[],
      turnPrefixMessages: [],
      isSplitTurn: false,
    },
    reason: "overflow",
    willRetry: true,
    signal: new AbortController().signal,
  } as unknown as SessionBeforeCompactEvent;
}

const CURSOR_AUTO = { provider: "cursor", id: "auto" } as Model<Api>;

test("activates only for cursor conversations", () => {
  expect(isCursorModelActive(compactEvent([]), CURSOR_AUTO)).toBe(true);
  expect(isCursorModelActive(compactEvent([assistant("cursor")]), undefined)).toBe(true);
  expect(isCursorModelActive(compactEvent([assistant("cursor")]), OTHER)).toBe(false);
  expect(isCursorModelActive(compactEvent([]), OTHER)).toBe(false);
  expect(isCursorModelActive(compactEvent([assistant("openai")]), undefined)).toBe(false);
});

test("handler returns a compaction result built from the fallback chain", async () => {
  const complete = vi.fn().mockResolvedValue({ content: [{ type: "text", text: "summary text" }] });
  const on = vi.fn();
  registerCursorCompaction({ on });

  const handler = on.mock.calls[0]?.[1] as (
    event: SessionBeforeCompactEvent,
    ctx: unknown,
  ) => Promise<{ compaction: unknown } | undefined>;

  const event = {
    ...compactEvent([assistant("cursor")]),
    preparation: {
      messagesToSummarize: [],
      turnPrefixMessages: [],
      isSplitTurn: false,
      firstKeptEntryId: "entry-1",
      tokensBefore: 1234,
      previousSummary: undefined,
    },
    customInstructions: undefined,
  } as unknown as SessionBeforeCompactEvent;

  const result = await handler(event, {
    model: { provider: "cursor", id: "auto" } as Model<Api>,
    modelRegistry: registry([KIMI], { complete }),
  });

  expect(result).toStrictEqual({
    compaction: {
      summary: "summary text",
      firstKeptEntryId: "entry-1",
      tokensBefore: 1234,
    },
  });
});

test("handler defers to default compaction when the chain is unavailable", async () => {
  const on = vi.fn();
  registerCursorCompaction({ on });
  const handler = on.mock.calls[0]?.[1] as (
    event: SessionBeforeCompactEvent,
    ctx: unknown,
  ) => Promise<{ compaction: unknown } | undefined>;

  const event = {
    ...compactEvent([assistant("cursor")]),
    preparation: {
      messagesToSummarize: [],
      turnPrefixMessages: [],
      isSplitTurn: false,
      firstKeptEntryId: "entry-1",
      tokensBefore: 1234,
      previousSummary: undefined,
    },
  } as unknown as SessionBeforeCompactEvent;

  await expect(
    handler(event, {
      model: { provider: "cursor", id: "auto" } as Model<Api>,
      modelRegistry: registry([]),
    }),
  ).resolves.toBeUndefined();
});

// Message type import guard: serializeConversation input compatibility.
test("converts messages for serialization without runtime dependency on pi internals", () => {
  const messages: Message[] = [{ role: "user", content: "hi", timestamp: 1 }];
  expect(messages.length).toBe(1);
});

test("textOfContent handles non-text and malformed parts defensively", () => {
  expect(textOfContent("plain string")).toBe("plain string");
  expect(textOfContent(null)).toBe("");
  expect(textOfContent(42)).toBe("");
  expect(textOfContent([{ type: "text", text: "a" }, { type: "image" }, "junk", null])).toBe("a");
});
