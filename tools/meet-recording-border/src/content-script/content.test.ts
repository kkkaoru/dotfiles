// Runs with bun: `bun run test` (vitest + happy-dom).
import { expect, test, vi } from "vitest";

test("the content script uses chrome.i18n labels and retires with the extension context", async () => {
  vi.useFakeTimers();
  const getMessage = vi.fn((key: string) => `msg:${key}`);
  const runtime: { id: string | undefined } = { id: "extension-id" };
  vi.stubGlobal("chrome", { i18n: { getMessage }, runtime });
  vi.stubGlobal("requestIdleCallback", (callback: IdleRequestCallback): number => {
    callback({ didTimeout: false, timeRemaining: () => 50 });
    return 0;
  });
  history.pushState(null, "", "/nxm-khwo-bro");
  await import("./content.ts");
  await vi.advanceTimersByTimeAsync(500);
  const hosts = [...document.documentElement.children].filter(
    (element) => element.tagName === "DIV",
  );
  expect(hosts.length).toBe(1);
  expect(getMessage.mock.calls).toStrictEqual([["loading"]]);
  runtime.id = undefined;
  document.dispatchEvent(new Event("visibilitychange"));
  await vi.advanceTimersByTimeAsync(500);
  expect(
    [...document.documentElement.children].filter((element) => element.tagName === "DIV").length,
  ).toBe(0);
  vi.clearAllTimers();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});
