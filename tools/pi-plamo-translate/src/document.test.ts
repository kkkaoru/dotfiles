// This TypeScript file is executed with Bun.
import { expect, test } from "vitest";
import { createServer, type ResponseLike, type ServerDeps } from "./server.ts";

interface TokenizeBody {
  readonly texts: readonly string[];
}

interface CompletionBody {
  readonly prompt: string;
  readonly max_tokens: number;
}

const response = (ok: boolean, body: unknown, status = 200): ResponseLike => ({
  ok,
  status,
  json: () => Promise.resolve(body),
});

const tokenizeBody = (body: string | undefined): TokenizeBody =>
  JSON.parse(body ?? "{}") as TokenizeBody;

const completionBody = (body: string | undefined): CompletionBody =>
  JSON.parse(body ?? "{}") as CompletionBody;

const deps = (fetch: ServerDeps["fetch"]): ServerDeps => ({
  exists: () => true,
  fetch,
  home: "/Users/x",
  now: () => 0,
  platform: "darwin",
  report: () => undefined,
  sleep: () => Promise.resolve(),
  spawn: () => ({ once: () => undefined, unref: () => undefined }),
});

// One token per character is what the Japanese tokenizer measures; the template counts too.
const counts = (texts: readonly string[]): ResponseLike =>
  response(true, { tokens: texts.map((text) => text.length), window: 2048, max_tokens: 4096 });

test("a document longer than the window is translated in chunks that each fit", async () => {
  const bodies: CompletionBody[] = [];
  const server = createServer(
    deps((url, init) => {
      if (url.endsWith("/v1/tokenize")) {
        return Promise.resolve(counts(tokenizeBody(init?.body).texts));
      }
      bodies.push(completionBody(init?.body));
      return Promise.resolve(response(true, { choices: [{ text: `T${String(bodies.length)}` }] }));
    }),
    18555,
  );
  const out = await server.complete("これは長い文書の一文です。".repeat(200), "ja");
  expect(bodies.length).toBeGreaterThan(1);
  for (const body of bodies) {
    // Template plus chunk plus the answer reserve stay inside the 2048-token window.
    expect(body.prompt.length).toBeLessThan(2048);
    expect(body.max_tokens).toBeGreaterThan(0);
  }
  expect(out).toBe(bodies.map((_body, index) => `T${String(index + 1)}`).join(""));
});

test("a sentence longer than the budget is cut and measured, not sent whole", async () => {
  const bodies: CompletionBody[] = [];
  const server = createServer(
    deps((url, init) => {
      if (url.endsWith("/v1/tokenize")) {
        return Promise.resolve(counts(tokenizeBody(init?.body).texts));
      }
      bodies.push(completionBody(init?.body));
      return Promise.resolve(response(true, { choices: [{ text: "x" }] }));
    }),
    18555,
  );
  await server.complete("あ".repeat(3000), "ja");
  expect(bodies.length).toBeGreaterThan(1);
  for (const body of bodies) {
    expect(body.prompt.length).toBeLessThan(2048);
  }
});

test("a server without a tokenizer keeps the single request", async () => {
  const bodies: CompletionBody[] = [];
  const server = createServer(
    deps((url, init) => {
      if (url.endsWith("/v1/tokenize")) {
        return Promise.resolve(response(false, {}, 501));
      }
      bodies.push(completionBody(init?.body));
      return Promise.resolve(response(true, { choices: [{ text: "hi" }] }));
    }),
    18555,
  );
  expect(await server.complete("あ。".repeat(400), "ja")).toBe("hi");
  expect(bodies).toHaveLength(1);
});
