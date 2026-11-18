// This TypeScript file is executed with Bun.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { expect, test } from "vitest";
import imageBudget from "./index.ts";
import { OMITTED } from "./src/budget.ts";

type Handler = (event: { readonly messages: readonly unknown[] }, ctx: unknown) => unknown;

function harness(): {
  readonly notes: string[];
  readonly run: (messages: unknown[], model: unknown) => unknown;
} {
  const notes: string[] = [];
  const captured: { handler: Handler | undefined } = { handler: undefined };
  const api = {
    on: (_name: string, handler: Handler) => {
      captured.handler = handler;
    },
  };
  imageBudget(api as unknown as ExtensionAPI);
  return {
    notes,
    run: (messages, model) =>
      captured.handler?.(
        { messages },
        { model, ui: { notify: (note: string) => notes.push(note) } },
      ),
  };
}

const images = (count: number): unknown[] => [
  {
    role: "toolResult",
    content: Array.from({ length: count }, (_unused, index) => ({
      type: "image",
      data: String(index),
      mimeType: "image/png",
    })),
  },
];

test("a model that declares an image cap keeps only the newest images", () => {
  const { notes, run } = harness();
  const messages = images(3);
  expect(run(messages, { inputLimits: { images: { maxPerRequest: 1 } } })).toStrictEqual({
    messages,
  });
  expect(JSON.stringify(messages)).toContain(OMITTED);
  expect(notes).toStrictEqual([
    "image-budget: 2 older images are left out of this request; the newest ones are sent.",
  ]);
  // Said once, so a long session is not flooded.
  run(images(3), { inputLimits: { images: { maxPerRequest: 1 } } });
  expect(notes).toHaveLength(1);
});

test("a model without a declared cap is left alone", () => {
  const { notes, run } = harness();
  const messages = images(3);
  expect(run(messages, {})).toBeUndefined();
  expect(JSON.stringify(messages)).not.toContain(OMITTED);
  expect(run(images(3), undefined)).toBeUndefined();
  expect(notes).toStrictEqual([]);
});

test("a declared per-message cap is enforced on its own", () => {
  const { notes, run } = harness();
  const messages = images(3);
  expect(run(messages, { inputLimits: { images: { maxPerMessage: 2 } } })).toStrictEqual({
    messages,
  });
  expect(JSON.stringify(messages).match(/"type":"image"/gu)).toHaveLength(2);
  expect(notes).toStrictEqual([
    "image-budget: 1 older images are left out of this request; the newest ones are sent.",
  ]);
});
