// This TypeScript file is executed with Bun.
import { Buffer } from "node:buffer";
import type { AssistantMessage } from "@earendil-works/pi-ai";
import { expect, it, vi } from "vitest";
import { summarizeBounded, summaryInputBudget } from "./bounded-summary.ts";

const RESPONSE: AssistantMessage = {
  role: "assistant",
  content: [{ type: "text", text: "handoff" }],
  api: "openai-responses",
  provider: "openai",
  model: "test",
  timestamp: 0,
  stopReason: "stop",
  usage: {
    input: 1,
    output: 2,
    cacheRead: 3,
    cacheWrite: 4,
    totalTokens: 10,
    reasoning: 1,
    cost: { input: 1, output: 2, cacheRead: 3, cacheWrite: 4, total: 10 },
  },
};
const SIGNAL: AbortSignal = new globalThis.AbortController().signal;
const TRUNCATION_MARKER = "\n\n[Segment summary truncated to the compaction budget.]";
const TRUNCATION_BYTES = Buffer.byteLength(TRUNCATION_MARKER, "utf8");

it("bounds each multilingual request and carries the running summary and complete source", async () => {
  const complete = vi.fn().mockResolvedValue(RESPONSE);
  const result = await summarizeBounded({
    text: "あ".repeat(8000),
    contextWindow: 24_000,
    signal: SIGNAL,
    complete,
  });
  expect(complete).toHaveBeenCalledTimes(8);
  expect(
    complete.mock.calls.every(
      ([prompt]: string[]) => Buffer.byteLength(prompt ?? "", "utf8") < 12_000,
    ),
  ).toBe(true);
  expect(complete.mock.calls[1]?.[0]).toMatch(/<previous-summary>\nhandoff\n/u);
  expect(
    complete.mock.calls
      .map(([prompt]: string[]) => prompt?.match(/<segment>\n([\s\S]*)\n<\/segment>/u)?.[1])
      .join("").length,
  ).toBe(8000);
  expect(result).toStrictEqual({
    text: "handoff",
    usage: {
      input: 8,
      output: 16,
      cacheRead: 24,
      cacheWrite: 32,
      totalTokens: 80,
      reasoning: 8,
      cost: { input: 8, output: 16, cacheRead: 24, cacheWrite: 32, total: 80 },
    },
  });
});

it("preserves non-BMP code points across bounded segment boundaries", async () => {
  const complete = vi.fn().mockResolvedValue(RESPONSE);
  await summarizeBounded({
    text: "😀".repeat(2001),
    contextWindow: 24_000,
    signal: SIGNAL,
    complete,
  });
  expect(complete).toHaveBeenCalledTimes(3);
  expect(
    complete.mock.calls
      .map(([prompt]: string[]) => prompt?.match(/<segment>\n([\s\S]*)\n<\/segment>/u)?.[1])
      .join(""),
  ).toMatch(/^😀{2001}$/u);
  expect(
    complete.mock.calls.every(
      ([prompt]: string[]) => Buffer.byteLength(prompt ?? "", "utf8") < 12_000,
    ),
  ).toBe(true);
});

it("caps large windows and rejects unusable window metadata", () => {
  expect(summaryInputBudget(1_000_000)).toBe(96_000);
  expect(() => summaryInputBudget(100)).toThrow("too small");
  expect(() => summaryInputBudget(Number.NaN)).toThrow("too small");
});

it("rejects empty history before any provider call", async () => {
  const complete = vi.fn();
  await expect(
    summarizeBounded({ text: "", contextWindow: 24_000, signal: SIGNAL, complete }),
  ).rejects.toThrow("non-empty history");
  expect(complete).not.toHaveBeenCalled();
});

it("recovers an xAI-sized truncated history instead of issuing tens of sequential calls", async () => {
  const complete = vi.fn().mockResolvedValue(RESPONSE);
  const result = await summarizeBounded({
    text: "x".repeat(80_000),
    contextWindow: 500_000,
    signal: SIGNAL,
    complete,
  });
  expect(complete).toHaveBeenCalledTimes(9);
  expect(result.text).toMatch(/^Emergency recovery compaction:/u);
});

it("automatically recovers histories beyond 8 chunks without unbounded calls", async () => {
  const complete = vi.fn().mockResolvedValue(RESPONSE);
  const onRecovery = vi.fn();
  const result = await summarizeBounded({
    text: `ORIGINAL GOAL\n${"x".repeat(2_000_000)}\nLATEST TASK`,
    contextWindow: 24_000,
    signal: SIGNAL,
    complete,
    onRecovery,
  });
  expect(complete).toHaveBeenCalledTimes(9);
  expect(onRecovery).toHaveBeenCalledOnce();
  expect(complete.mock.calls[0]?.[0]).toMatch(/ORIGINAL GOAL/u);
  expect(complete.mock.lastCall?.[0]).toMatch(/LATEST TASK/u);
  expect(
    complete.mock.calls.every(
      ([prompt]: string[]) => Buffer.byteLength(prompt ?? "", "utf8") < 12_000,
    ),
  ).toBe(true);
  expect(result.text).toMatch(
    /^Emergency recovery compaction:[\s\S]*original session history is preserved[\s\S]*handoff$/u,
  );
  expect(result.usage.totalTokens).toBe(90);
});

it("recovers without a notification callback and rejects failed recovery without partial output", async () => {
  const complete = vi.fn().mockResolvedValue(RESPONSE);
  const result = await summarizeBounded({
    text: "x".repeat(129_000),
    contextWindow: 24_000,
    signal: SIGNAL,
    complete,
  });
  expect(result.text).toMatch(/^Emergency recovery compaction:/u);
  complete.mockRejectedValueOnce(new Error("provider unavailable"));
  await expect(
    summarizeBounded({
      text: "x".repeat(129_000),
      contextWindow: 24_000,
      signal: SIGNAL,
      complete,
    }),
  ).rejects.toThrow("provider unavailable");
});

it("honors cancellation before and after a provider call", async () => {
  const controller = new globalThis.AbortController();
  controller.abort();
  const complete = vi.fn();
  await expect(
    summarizeBounded({ text: "text", contextWindow: 24_000, signal: controller.signal, complete }),
  ).rejects.toThrow();
  expect(complete).not.toHaveBeenCalled();
  const pending = new globalThis.AbortController();
  complete.mockImplementation(async () => {
    pending.abort();
    return RESPONSE;
  });
  await expect(
    summarizeBounded({ text: "text", contextWindow: 24_000, signal: pending.signal, complete }),
  ).rejects.toThrow();
});

it.each(["error", "aborted", "toolUse"] satisfies AssistantMessage["stopReason"][])(
  "rejects %s rather than saving a partial compaction",
  async (stopReason) => {
    const complete = vi
      .fn()
      .mockResolvedValue({ ...RESPONSE, stopReason, errorMessage: "provider failure" });
    await expect(
      summarizeBounded({ text: "text", contextWindow: 24_000, signal: SIGNAL, complete }),
    ).rejects.toThrow("provider failure");
  },
);

it("retries connection drops then keeps the recovered segment", async () => {
  const failed: AssistantMessage = {
    ...RESPONSE,
    stopReason: "error",
    errorMessage: "Connection error.",
    content: [],
  };
  const complete = vi.fn().mockResolvedValueOnce(failed).mockResolvedValueOnce(RESPONSE);
  const result = await summarizeBounded({
    text: "text",
    contextWindow: 24_000,
    signal: SIGNAL,
    complete,
  });
  expect(complete).toHaveBeenCalledTimes(2);
  expect(result.text).toBe("handoff");
});

it("gives up after repeated connection errors", async () => {
  const complete = vi.fn().mockResolvedValue({
    ...RESPONSE,
    stopReason: "error",
    errorMessage: "Retry failed after 3 attempts: Connection error.",
    content: [],
  });
  await expect(
    summarizeBounded({ text: "text", contextWindow: 24_000, signal: SIGNAL, complete }),
  ).rejects.toThrow("Connection error");
  expect(complete).toHaveBeenCalledTimes(3);
});

it("keeps a length-truncated summary instead of aborting compaction", async () => {
  const complete = vi.fn().mockResolvedValue({
    ...RESPONSE,
    stopReason: "length",
    content: [{ type: "text", text: "partial handoff" }],
  });
  const result = await summarizeBounded({
    text: "text",
    contextWindow: 24_000,
    signal: SIGNAL,
    complete,
  });
  expect(result.text).toBe("partial handoff");
});

it("keeps thinking-only segment content instead of failing the segment", async () => {
  const complete = vi.fn().mockResolvedValue({
    ...RESPONSE,
    content: [{ type: "thinking", thinking: "reasoned handoff" }],
  });
  const result = await summarizeBounded({
    text: "text",
    contextWindow: 24_000,
    signal: SIGNAL,
    complete,
  });
  expect(result.text).toBe("reasoned handoff");
});

it("keeps earlier segments when a later segment returns no content", async () => {
  const complete = vi
    .fn()
    .mockResolvedValueOnce(RESPONSE)
    .mockResolvedValueOnce({ ...RESPONSE, content: [] });
  const result = await summarizeBounded({
    text: "x".repeat(1500),
    contextWindow: 24_000,
    signal: SIGNAL,
    complete,
  });
  expect(complete).toHaveBeenCalledTimes(2);
  expect(result.text).toBe("handoff");
});

it("rejects tool calls, empty output and truncates an overlong generated summary", async () => {
  const complete = vi
    .fn()
    .mockResolvedValueOnce({
      ...RESPONSE,
      content: [{ type: "toolCall", id: "tool", name: "bash", arguments: {} }],
    })
    .mockResolvedValueOnce({ ...RESPONSE, content: [] })
    .mockResolvedValueOnce({ ...RESPONSE, content: [{ type: "text", text: "x".repeat(3001) }] });
  await expect(
    summarizeBounded({ text: "text", contextWindow: 24_000, signal: SIGNAL, complete }),
  ).rejects.toThrow("stop");
  await expect(
    summarizeBounded({ text: "text", contextWindow: 24_000, signal: SIGNAL, complete }),
  ).rejects.toThrow("empty summary");
  const truncated = await summarizeBounded({
    text: "text",
    contextWindow: 24_000,
    signal: SIGNAL,
    complete,
  });
  expect(truncated.text).toBe(`${"x".repeat(3000 - TRUNCATION_BYTES)}${TRUNCATION_MARKER}`);
});

it("bounds a runaway summary on a code-point boundary instead of aborting", async () => {
  const complete = vi.fn().mockResolvedValue({
    ...RESPONSE,
    content: [{ type: "text", text: "😀".repeat(3000) }],
  });
  const result = await summarizeBounded({
    text: "text",
    contextWindow: 24_000,
    signal: SIGNAL,
    complete,
  });
  expect(Buffer.byteLength(result.text, "utf8")).toBeLessThanOrEqual(3000);
  expect(result.text.endsWith(TRUNCATION_MARKER)).toBe(true);
  expect(result.text).not.toMatch(/[\uD800-\uDBFF]$/u);
});

it("handles providers without reasoning usage and joins only text blocks", async () => {
  const complete = vi.fn().mockResolvedValue({
    ...RESPONSE,
    usage: { ...RESPONSE.usage, reasoning: undefined },
    content: [
      { type: "thinking", thinking: "private" },
      { type: "text", text: "one" },
      { type: "text", text: "two" },
    ],
  });
  const result = await summarizeBounded({
    text: "text",
    contextWindow: 24_000,
    signal: SIGNAL,
    complete,
  });
  expect(result.text).toBe("one\ntwo");
  expect(result.usage.reasoning).toBeUndefined();
});
