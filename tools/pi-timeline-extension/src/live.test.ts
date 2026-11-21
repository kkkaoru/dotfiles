// This TypeScript file is executed with Bun.
import { expect, test } from "vitest";
import { plamoDisplay } from "./live.ts";
import { TranslatedTimeline } from "./translate.ts";

const settings =
  (replyLanguage: string): (() => string) =>
  () =>
    JSON.stringify({ "pi-plamo-translate": { replyLanguage } });
test("a reader whose language is off keeps the tree in English", () => {
  expect(plamoDisplay(settings("off"))).toBeUndefined();
});

test("the reader's language and port come from pi-plamo-translate's settings", () => {
  expect(plamoDisplay(settings("ja"))).toBeInstanceOf(TranslatedTimeline);
});
