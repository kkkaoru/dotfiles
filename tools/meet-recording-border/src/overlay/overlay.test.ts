// Runs with bun: `bun run test` (vitest + happy-dom).
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import englishMessages from "../../_locales/en/messages.json" with { type: "json" };
import japaneseMessages from "../../_locales/ja/messages.json" with { type: "json" };
import { type MessageKey, type View, compilePatterns } from "../detection/state/meet-state.ts";
import englishPhrases from "../detection/phrases/en.json" with { type: "json" };
import japanesePhrases from "../detection/phrases/ja.json" with { type: "json" };
import { type JoinState, createOverlay, renderView, start, tick } from "./overlay.ts";

const patterns = compilePatterns([englishPhrases, japanesePhrases]);
function english(key: MessageKey): string {
  return englishMessages[key].message;
}
function japanese(key: MessageKey): string {
  return japaneseMessages[key].message;
}
function overlayHosts(): Element[] {
  return [...document.documentElement.children].filter((element) => element.tagName === "DIV");
}

beforeEach(() => {
  document.body.innerHTML = "";
  history.pushState(null, "", "/nxm-khwo-bro");
});

afterEach(() => {
  overlayHosts().map((element) => element.remove());
  vi.useRealTimers();
});

test("createOverlay builds a hidden, detached overlay inside a closed shadow root", () => {
  const overlay = createOverlay(document);
  expect(overlay.host.isConnected).toBe(false);
  expect(overlay.host.shadowRoot).toBe(null);
  expect(overlay.frame.hidden).toBe(true);
  expect(overlay.frame.className).toBe("frame");
  expect(overlay.label.className).toBe("label");
  expect(overlay.label.parentElement).toBe(overlay.frame);
  expect(overlay.label.textContent).toBe("");
});

function propertyNames(style: CSSStyleDeclaration | undefined): string[] {
  return style === undefined
    ? []
    : Array.from({ length: style.length }, (_value, index) => style.item(index));
}
function styleRules(overlay: ReturnType<typeof createOverlay>): CSSStyleRule[] {
  return [...(overlay.frame.parentNode?.querySelector("style")?.sheet?.cssRules ?? [])].filter(
    (rule) => rule instanceof CSSStyleRule,
  );
}

test("the :host scope only defines the custom properties", () => {
  const overlay = createOverlay(document);
  document.documentElement.append(overlay.host);
  const host = styleRules(overlay).find((rule) => rule.selectorText === ":host");
  expect(propertyNames(host?.style).every((name) => name.startsWith("--"))).toBe(true);
  expect(host?.style.getPropertyValue("--border-width")).toBe("12px");
  expect(host?.style.getPropertyValue("--corner-radius")).toBe("26px");
  expect(host?.style.getPropertyValue("--pulse-duration")).toBe("1600ms");
  expect(host?.style.getPropertyValue("--color-recording")).toBe("#ea4335");
  expect(
    propertyNames(host?.style)
      .map((name) => host?.style.getPropertyValue(name) ?? "")
      .filter((value) => /var\(/u.test(value)),
  ).toStrictEqual([]);
});

test("the frame styles define no custom properties", () => {
  const overlay = createOverlay(document);
  document.documentElement.append(overlay.host);
  const frame = styleRules(overlay).find((rule) => rule.selectorText === ".frame");
  expect(propertyNames(frame?.style).some((name) => name.startsWith("--"))).toBe(false);
  expect(frame?.style.getPropertyValue("will-change")).toBe("opacity");
});

// Happy-dom does not resolve var() chains inherited from :host; Chrome resolves the rest.
// These tests check that the frame matches each tone rule and what that rule assigns.
function toneRule(
  overlay: ReturnType<typeof createOverlay>,
  tone: string,
): CSSStyleRule | undefined {
  return styleRules(overlay).find((rule) => rule.selectorText === `.frame[data-tone="${tone}"]`);
}

test("the tone switches the border color", () => {
  const overlay = createOverlay(document);
  document.documentElement.append(overlay.host);
  renderView({ overlay, view: { tone: "recording", messages: ["recording"] }, translate: english });
  expect(overlay.frame.matches('.frame[data-tone="recording"]')).toBe(true);
  expect(toneRule(overlay, "recording")?.style.getPropertyValue("--color")).toBe(
    "var(--color-recording)",
  );
  renderView({ overlay, view: { tone: "gemini", messages: ["gemini"] }, translate: english });
  expect(overlay.frame.matches('.frame[data-tone="gemini"]')).toBe(true);
  expect(toneRule(overlay, "gemini")?.style.getPropertyValue("--color")).toBe(
    "var(--color-gemini)",
  );
  renderView({ overlay, view: { tone: "idle", messages: ["idle"] }, translate: english });
  expect(overlay.frame.matches('.frame[data-tone="idle"]')).toBe(true);
  expect(toneRule(overlay, "idle")?.style.getPropertyValue("--color")).toBe("var(--color-idle)");
});

test("the loading tone stops the pulse through a variable", () => {
  const overlay = createOverlay(document);
  document.documentElement.append(overlay.host);
  renderView({ overlay, view: { tone: "loading", messages: ["loading"] }, translate: english });
  expect(getComputedStyle(overlay.frame).getPropertyValue("--pulse-name")).toBe("none");
  expect(toneRule(overlay, "loading")?.style.getPropertyValue("--color")).toBe(
    "var(--color-loading)",
  );
});

test("renderView shows the frame with a tone and English labels", () => {
  const overlay = createOverlay(document);
  renderView({
    overlay,
    view: { tone: "recording", messages: ["recording", "gemini"] },
    translate: english,
  });
  expect(overlay.frame.hidden).toBe(false);
  expect(overlay.frame.dataset["tone"]).toBe("recording");
  expect(overlay.label.textContent).toBe("● Recording + Gemini taking notes");
});

test("renderView paints Japanese labels", () => {
  const overlay = createOverlay(document);
  renderView({
    overlay,
    view: { tone: "transcription", messages: ["transcription", "gemini"] },
    translate: japanese,
  });
  expect(overlay.frame.dataset["tone"]).toBe("transcription");
  expect(overlay.label.textContent).toBe("文字起こし中 + Gemini メモ作成中");
});

test("renderView leaves the DOM untouched when the view is unchanged", () => {
  const overlay = createOverlay(document);
  const view: View = { tone: "idle", messages: ["idle"] };
  renderView({ overlay, view, translate: english });
  const observer = new MutationObserver(() => undefined);
  observer.observe(overlay.frame, {
    attributes: true,
    childList: true,
    subtree: true,
    characterData: true,
  });
  renderView({ overlay, view, translate: english });
  expect(observer.takeRecords().length).toBe(0);
  observer.disconnect();
});

test("English and Japanese locales define the same messages", () => {
  expect(Object.keys(englishMessages)).toStrictEqual([
    "name",
    "description",
    "loading",
    "idle",
    "recording",
    "transcription",
    "gemini",
  ]);
  expect(Object.keys(japaneseMessages)).toStrictEqual([
    "name",
    "description",
    "loading",
    "idle",
    "recording",
    "transcription",
    "gemini",
  ]);
  expect(japanese("idle")).toBe("録画なし");
  expect(english("idle")).toBe("Not recording");
});

test("tick attaches the overlay but hides it outside a meeting page", () => {
  history.pushState(null, "", "/home");
  const overlay = createOverlay(document);
  overlay.frame.hidden = false;
  tick({
    page: document,
    overlay,
    state: { hint: [], joinedAt: 0 },
    now: 1000,
    translate: english,
    patterns,
  });
  expect(overlay.host.parentElement).toBe(document.documentElement);
  expect(overlay.frame.hidden).toBe(true);
});

test("tick shows loading before joining and remembers the pre-join notice", () => {
  document.body.innerHTML = "<div>Meet は録画を行います</div>";
  const overlay = createOverlay(document);
  const state: JoinState = { hint: [], joinedAt: 0 };
  expect(tick({ page: document, overlay, state, now: 1000, translate: english, patterns })).toBe(
    undefined,
  );
  expect(overlay.frame.dataset["tone"]).toBe("loading");
  expect(overlay.label.textContent).toBe("Before joining (checking status)");
  expect(state.hint).toStrictEqual(["recording"]);
  expect(state.joinedAt).toBe(0);
});

test("tick uses the pre-join notice right after joining", () => {
  document.body.innerHTML = "<button><i>call_end</i></button>";
  const overlay = createOverlay(document);
  const state: JoinState = { hint: ["recording"], joinedAt: 0 };
  expect(tick({ page: document, overlay, state, now: 5000, translate: japanese, patterns })).toBe(
    10_000,
  );
  expect(state.joinedAt).toBe(5000);
  expect(overlay.frame.dataset["tone"]).toBe("recording");
  expect(overlay.label.textContent).toBe("● 録画中");
});

test("tick keeps the join time and drops the notice after the grace period", () => {
  document.body.innerHTML = "<button><i>call_end</i></button>";
  const overlay = createOverlay(document);
  const state: JoinState = { hint: ["recording"], joinedAt: 5000 };
  expect(tick({ page: document, overlay, state, now: 15_000, translate: english, patterns })).toBe(
    undefined,
  );
  expect(state.joinedAt).toBe(5000);
  expect(overlay.frame.dataset["tone"]).toBe("idle");
  expect(overlay.label.textContent).toBe("Not recording");
});

test("tick resets the join time after leaving the call", () => {
  const overlay = createOverlay(document);
  const state: JoinState = { hint: [], joinedAt: 5000 };
  tick({ page: document, overlay, state, now: 20_000, translate: english, patterns });
  expect(state.joinedAt).toBe(0);
  expect(overlay.label.textContent).toBe("Before joining (checking status)");
});

test("tick does not attach the overlay twice", () => {
  const overlay = createOverlay(document);
  tick({
    page: document,
    overlay,
    state: { hint: [], joinedAt: 0 },
    now: 1000,
    translate: english,
    patterns,
  });
  tick({
    page: document,
    overlay,
    state: { hint: [], joinedAt: 0 },
    now: 1500,
    translate: english,
    patterns,
  });
  expect(overlayHosts().length).toBe(1);
});

test("tick asks for a recheck when the join grace period ends", () => {
  document.body.innerHTML = "<button><i>call_end</i></button>";
  const overlay = createOverlay(document);
  const state: JoinState = { hint: [], joinedAt: 5000 };
  expect(tick({ page: document, overlay, state, now: 11_000, translate: english, patterns })).toBe(
    4000,
  );
});

test("start renders once the page watcher runs its first check", async () => {
  vi.useFakeTimers();
  vi.stubGlobal("requestIdleCallback", (callback: IdleRequestCallback): number => {
    callback({ didTimeout: false, timeRemaining: () => 50 });
    return 0;
  });
  document.body.innerHTML = "<button><i>call_end</i></button><div>録画中</div>";
  const translate = vi.fn(english);
  const watcher = start({ page: document, translate, patterns, isAlive: () => true });
  expect(overlayHosts().length).toBe(0);
  await vi.advanceTimersByTimeAsync(500);
  expect(overlayHosts().length).toBe(1);
  expect(translate.mock.calls).toStrictEqual([["recording"]]);
  watcher.stop();
  vi.unstubAllGlobals();
});

test("start retires once the extension context is invalidated", async () => {
  vi.useFakeTimers();
  vi.stubGlobal("requestIdleCallback", (callback: IdleRequestCallback): number => {
    callback({ didTimeout: false, timeRemaining: () => 50 });
    return 0;
  });
  document.body.innerHTML = "<button><i>call_end</i></button>";
  const translate = vi.fn(english);
  const isAlive = vi.fn().mockReturnValueOnce(true).mockReturnValue(false);
  start({ page: document, translate, patterns, isAlive });
  await vi.advanceTimersByTimeAsync(500);
  expect(overlayHosts().length).toBe(1);
  document.dispatchEvent(new Event("visibilitychange"));
  await vi.advanceTimersByTimeAsync(500);
  expect(overlayHosts().length).toBe(0);
  document.dispatchEvent(new Event("visibilitychange"));
  await vi.advanceTimersByTimeAsync(500);
  expect(isAlive.mock.calls.length).toBe(2);
  expect(translate.mock.calls).toStrictEqual([["idle"]]);
  vi.unstubAllGlobals();
});
