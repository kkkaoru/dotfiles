/* oxlint-disable capitalized-comments, no-await-in-loop, no-console, no-undef, prefer-destructuring, typescript/dot-notation, typescript/no-unnecessary-type-assertion, unicorn/no-process-exit */
// Measures the chunked path against a real document longer than the model's window.
//
//   bun run scripts/measure-document.ts [sentences]
//
// Prints the single-shot request and, when the server can count tokens, the planned chunks with
// the prompt tokens each one preflights. The server must be answering on PLAMO_PORT (18555).
//
// A measurement script: printing its numbers is the point, the requests are measured one at a
// time so each latency belongs to its own chunk, and it runs under Bun, which supplies the globals
// this package's Node-free lint config does not declare.

import { plamoPrompt } from "../../claude-plamo-translate/hooks/translate.ts";
import { plan, tokenCounts, type Post } from "../src/chunk.ts";
import { completionText, type ResponseLike } from "../src/server.ts";

const port = Number(process.env["PLAMO_PORT"] ?? 18555);
const base = `http://127.0.0.1:${String(port)}`;

const post: Post = async (path, body) => {
  const response = await fetch(`${base}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  return {
    ok: response.ok,
    status: response.status,
    json: () => response.json() as Promise<unknown>,
  } satisfies ResponseLike;
};

const sentence =
  "キャッシュを再利用すると、同じプロンプトの再計算を避けられます。文を分割して窓に収めると、" +
  "長い文書でも翻訳が途中で切れません。";

const sentences = Number(process.argv[2] ?? 60);
const document = sentence.repeat(sentences);

const template = await tokenCounts(post, [document]);
if (template === null) {
  console.error("the server has no /v1/tokenize: restart it on the current build");
  process.exit(1);
}

const window = template.window;
const promptTokens = template.counts[0] ?? 0;
console.log(`document: ${String(document.length)} chars, ${String(promptTokens)} prompt tokens`);
console.log(`window:   ${String(window)} tokens`);

const complete = async (text: string, maxTokens: number): Promise<string> => {
  const response = await post("/v1/completions", {
    prompt: plamoPrompt(text, "en"),
    max_tokens: maxTokens,
    temperature: 0,
    stop: ["<|plamo:tag|>", "<|plamo:bos|>"],
  });
  if (!response.ok) {
    throw new Error(`completion failed: ${String(response.status)}`);
  }
  return completionText(await response.json()) ?? "";
};

/** What the mod asked for before chunking: one answer as long as the model's own limit. */
const SINGLE_MAX_TOKENS = 4096;

// The single request a text of this length used to send: the prefill already fills most of the
// window, and the answer still asks for 4096 tokens, so the decode runs past it.
const singleStart = performance.now();
const single = await complete(document, SINGLE_MAX_TOKENS);
const singleSeconds = (performance.now() - singleStart) / 1000;
console.log(
  `\nsingle request: prompt ${String(promptTokens)} + max_tokens ${String(SINGLE_MAX_TOKENS)} = ` +
    `${String(promptTokens + SINGLE_MAX_TOKENS)} wanted of ${String(window)}; ` +
    `${String(single.length)} chars back in ${singleSeconds.toFixed(1)} s`,
);
console.log(`  tail: ${JSON.stringify(single.slice(-40))}`);

const planned = await plan(post, document, "ja", 4096);
if (planned === null) {
  console.log("\nthe planner kept the single request");
  process.exit(0);
}

console.log(`\nchunked: ${String(planned.chunks.length)} requests`);
let answerChars = 0;
const chunkStart = performance.now();
for (const [index, chunk] of planned.chunks.entries()) {
  const counts = await tokenCounts(post, [chunk.text]);
  const start = performance.now();
  const answer = await complete(chunk.text, chunk.maxTokens);
  const seconds = (performance.now() - start) / 1000;
  answerChars += answer.length;
  console.log(
    `  ${String(index + 1)}/${String(planned.chunks.length)}: prompt ${String(counts?.counts[0] ?? 0)}` +
      ` + answer ${String(chunk.maxTokens)} = ${String((counts?.counts[0] ?? 0) + chunk.maxTokens)}` +
      ` of ${String(window)} tokens, ${answer.length} chars in ${seconds.toFixed(1)} s`,
  );
}
const chunkSeconds = (performance.now() - chunkStart) / 1000;
console.log(
  `\nchunked total: ${String(answerChars)} chars in ${chunkSeconds.toFixed(1)} s ` +
    `(single request: ${String(single.length)} chars in ${singleSeconds.toFixed(1)} s)`,
);
