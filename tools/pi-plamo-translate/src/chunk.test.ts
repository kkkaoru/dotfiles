// This TypeScript file is executed with Bun.
import { expect, test } from "vitest";
import { plamoPrompt } from "../../claude-plamo-translate/hooks/translate.ts";
import { pack, plan, tokenCounts, units } from "./chunk.ts";
import type { ResponseLike } from "./server.ts";

test("units concatenate back to the input, separators included", () => {
  const text = "一文目です。二文目！\nThird one. Fourth?  」おわり。";
  expect(units(text).join("")).toBe(text);
});

test("units split Japanese and English sentences", () => {
  expect(units("あ。い！う？")).toStrictEqual(["あ。", "い！", "う？"]);
  expect(units("One. Two? Three!")).toStrictEqual(["One. ", "Two? ", "Three!"]);
});

test("units keep a closing quote with its sentence", () => {
  expect(units("「終わり。」次。")).toStrictEqual(["「終わり。」", "次。"]);
});

test("units do not split a decimal point inside a sentence", () => {
  expect(units("Pi is 3.14 exactly.")).toStrictEqual(["Pi is 3.14 exactly."]);
});

test("units return the whole text when it has no terminator", () => {
  expect(units("no terminator here")).toStrictEqual(["no terminator here"]);
});

test("pack groups consecutive units within the budget", () => {
  expect(pack([10, 10, 10, 10], 25)).toStrictEqual([
    [0, 1],
    [2, 3],
  ]);
});

test("pack gives a unit over the budget a group of its own", () => {
  // `fit` splits those before packing; a group of one is the shape it must not overflow.
  expect(pack([5, 40, 5], 20)).toStrictEqual([[0], [1], [2]]);
});

test("pack of nothing is nothing", () => {
  expect(pack([], 20)).toStrictEqual([]);
});

interface TokenizeBody {
  readonly texts: readonly string[];
}

const response = (ok: boolean, body: unknown): ResponseLike => ({
  ok,
  status: ok ? 200 : 500,
  json: () => Promise.resolve(body),
});

const tokenizeBody = (body: unknown): TokenizeBody => body as TokenizeBody;

/** One token per character, the Japanese tokenizer's own measure; the template counts too. */
const counting =
  (window: number) =>
  (path: string, body: unknown): Promise<ResponseLike> => {
    expect(path).toBe("/v1/tokenize");
    const { texts } = tokenizeBody(body);
    return Promise.resolve(response(true, { tokens: texts.map((text) => text.length), window }));
  };

test("tokenCounts returns the counts and the window", async () => {
  expect(await tokenCounts(counting(2048), ["ab", "c"])).toStrictEqual({
    counts: [2, 1],
    window: 2048,
  });
});

test("tokenCounts is null when the server refuses, throws or answers another shape", async () => {
  expect(await tokenCounts(() => Promise.resolve(response(false, {})), ["a"])).toBeNull();
  expect(await tokenCounts(() => Promise.reject(new Error("closed")), ["a"])).toBeNull();
  expect(await tokenCounts(() => Promise.resolve(response(true, [])), ["a"])).toBeNull();
  expect(
    await tokenCounts(() => Promise.resolve(response(true, { tokens: [1], window: "big" })), ["a"]),
  ).toBeNull();
  expect(
    await tokenCounts(
      () => Promise.resolve(response(true, { tokens: [1], window: 8 })),
      ["a", "b"],
    ),
  ).toBeNull();
});

test("plan keeps a short text on the single request", async () => {
  expect(await plan(counting(2048), "短い。", "ja", 4096)).toBeNull();
});

test("plan keeps the single request when the server cannot count", async () => {
  expect(
    await plan(() => Promise.resolve(response(false, {})), "あ。".repeat(300), "ja", 4096),
  ).toBeNull();
});

test("plan keeps the single request when the window has no room for the source", async () => {
  // The template alone fills this window, so there is no budget to split against.
  expect(await plan(counting(10), "あ。".repeat(300), "ja", 4096)).toBeNull();
});

test("plan keeps the single request when the text already fits one chunk", async () => {
  expect(await plan(counting(2048), "あ".repeat(600), "ja", 4096)).toBeNull();
});

test("plan gives up when a cut cannot be measured", async () => {
  const post = (path: string, body: unknown): Promise<ResponseLike> => {
    const { texts } = tokenizeBody(body);
    // The template and the pieces count; the two halves of a cut do not.
    return Promise.resolve(texts.length === 2 ? response(false, {}) : counting(2048)(path, body));
  };
  expect(await plan(post, "あ".repeat(3000), "ja", 4096)).toBeNull();
});

test("plan splits a document into chunks that each fit and rejoins with spaces", async () => {
  const planned = await plan(counting(2048), "One. ".repeat(300), "en", 4096);
  expect(planned).not.toBeNull();
  expect(planned?.chunks.length).toBeGreaterThan(1);
  expect(planned?.join).toBe(" ");
  for (const chunk of planned?.chunks ?? []) {
    expect(chunk.text.length).toBeLessThanOrEqual(1004);
    expect(chunk.maxTokens).toBeGreaterThan(0);
  }
  expect((planned?.chunks ?? []).map((chunk) => chunk.text).join("")).toBe("One. ".repeat(300));
});

test("tokenCounts is null when a count is not a number", async () => {
  expect(
    await tokenCounts(
      () => Promise.resolve(response(true, { tokens: [1, "x"], window: 8 })),
      ["a", "b"],
    ),
  ).toBeNull();
});

test("plan keeps the single request when a unit cannot be cut any further", async () => {
  // Two tokens per character and a budget of one: the cut rounds down to nothing, so the text
  // Stays whole rather than being sent in a request that would overflow.
  const post = (_path: string, body: unknown): Promise<ResponseLike> => {
    const { texts } = tokenizeBody(body);
    const window = 2 * plamoPrompt("", "ja").length + 2;
    return Promise.resolve(
      response(true, { tokens: texts.map((text) => text.length * 2), window }),
    );
  };
  expect(await plan(post, "あ".repeat(600), "ja", 4096)).toBeNull();
});

test("plan keeps the single request when the pieces cannot be counted", async () => {
  const post = (path: string, body: unknown): Promise<ResponseLike> => {
    const { texts } = tokenizeBody(body);
    const template = texts.join("").includes("<|plamo:tag|>");
    return Promise.resolve(template ? counting(2048)(path, body) : response(false, {}));
  };
  expect(await plan(post, "あ。".repeat(300), "ja", 4096)).toBeNull();
});
