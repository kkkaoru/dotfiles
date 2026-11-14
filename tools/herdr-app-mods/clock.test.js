import { existsSync } from "node:fs";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

import { host, reset } from "./mocks/herdr.js";
import {
  ClockCat,
  DRACULA,
  DULL_RAINBOWS,
  HOUR_RAINBOWS,
  clockLook,
  clockStrip,
  clockText,
  sidebarWidth,
} from "../../.config/herdr-app/mods/tabs-clock/clock.js";

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"], now: new Date(2026, 10, 14, 18, 25, 30, 40) });
});

afterEach(() => {
  reset();
  vi.useRealTimers();
});

const nodes = (el) => (el?.kids ? [el, ...el.kids.flatMap(nodes)] : []);
const paths = (el) =>
  nodes(el)
    .filter((n) => n.path)
    .map((n) => n.path);
const color = (el) => el.calls.find(([k]) => k === "text_color")?.[1];
const arg = (el, name) => el.calls.find(([k]) => k === name)?.[1];
const LOOK = { time: "18:25:30", r: 5, ink: "#bd93f9", fur: 2, taste: 3, progress: 0.25 };

test("the time reads to the second, zero-padded", () => {
  expect(clockText(new Date(2026, 0, 1, 7, 4, 3))).toBe("07:04:03");
  expect(clockText(new Date(2026, 0, 1, 23, 59, 59))).toBe("23:59:59");
});

test("each hour has its own rainbow and Dracula color, and each minute its own cat", () => {
  // 24 different rainbows, none of the dull ones, each with its frames.
  expect(new Set(HOUR_RAINBOWS).size).toBe(24);
  expect(HOUR_RAINBOWS.filter((r) => DULL_RAINBOWS.includes(r))).toStrictEqual([]);
  for (const r of HOUR_RAINBOWS) {
    const frame = new URL(
      `../../.config/herdr-app/mods/tabs-clock/nyan/trail-${r}-0.svg`,
      import.meta.url,
    );
    expect(existsSync(frame)).toBe(true);
  }
  expect(new Set(DRACULA).size).toBe(8);
  expect(clockLook(new Date(2026, 0, 1, 0, 9, 59))).toStrictEqual({
    time: "00:09:59",
    r: 16,
    ink: "#ff79c6",
    fur: 3,
    taste: 4,
    progress: 59 / 60,
  });
  expect(clockLook(new Date(2026, 0, 1, 18, 25, 0))).toMatchObject({
    r: 24,
    ink: "#ffb86c",
    fur: 1,
    taste: 5,
  });
  // The next minute's cat differs in fur and poptart, into the next hour too; minutes 0–35 are
  // all 36 cats.
  const cats = Array.from({ length: 60 }, (_v, m) => clockLook(new Date(2026, 0, 1, 9, m)));
  expect(
    cats.every((c, m) => c.fur !== cats[(m + 1) % 60].fur && c.taste !== cats[(m + 1) % 60].taste),
  ).toBe(true);
  expect(new Set(cats.slice(0, 36).map((c) => `${c.fur}${c.taste}`)).size).toBe(36);
  // The cat's way along the bar: none at :00, half at :30.5, almost all just before the minute.
  expect(
    [0, 30_500, 59_999].map(
      (ms) => clockLook(new Date(2026, 0, 1, 9, 0, Math.floor(ms / 1000), ms % 1000)).progress,
    ),
  ).toStrictEqual([0, 30_500 / 60_000, 59_999 / 60_000]);
  // Neighboring hours never share a color, across midnight too.
  const inks = Array.from({ length: 24 }, (_v, h) => clockLook(new Date(2026, 0, 1, h)).ink);
  expect(inks.every((ink, h) => ink !== inks[(h + 1) % 24])).toBe(true);
});

test("every frame the clock names exists, without the night sky", () => {
  const strip = clockStrip({ ...LOOK, r: 23 }, 11, 0);
  for (const path of paths(strip)) {
    expect(existsSync(new URL(`../../.config/herdr-app/${path}`, import.meta.url))).toBe(true);
  }
});

test("the cat is its progress along the bar, its rainbow trailing to the left edge, the time over it", () => {
  const strip = clockStrip(LOOK, 7, 268);
  const [preload, gap, rainbow, cat, ahead, text] = strip.kids;
  // All twelve frames of this rainbow, loaded out of sight.
  expect(arg(preload, "opacity")).toBe(0);
  expect(paths(preload)).toHaveLength(24);
  expect(paths(preload).slice(0, 2)).toStrictEqual([
    "mods/tabs-clock/nyan/trail-5-0.svg",
    "mods/tabs-clock/nyan/cat-5-0.svg",
  ]);
  // The sidebar's width empty; then the bar less the cat, a quarter rainbow (ending at the cat)
  // and three quarters still ahead of it.
  expect(arg(gap, "w")).toBe(268);
  expect([rainbow, ahead].map((el) => [arg(el, "flex_basis"), arg(el, "flex_grow")])).toStrictEqual(
    [
      [0, 0.25],
      [0, 0.75],
    ],
  );
  expect(rainbow.calls).toContainEqual(["justify_end"]);
  expect(paths(rainbow)).toStrictEqual(
    Array.from({ length: 16 }, () => "mods/tabs-clock/nyan/trail-5-7.svg"),
  );
  expect(paths(cat)).toStrictEqual([
    "mods/tabs-clock/nyan/cat-5-7.svg",
    "mods/tabs-clock/nyan/fur-7.svg",
    "mods/tabs-clock/nyan/frosting-7.svg",
    "mods/tabs-clock/nyan/sprinkles-7.svg",
  ]);
  // White fur; lemon frosting with orange sprinkles.
  expect(cat.kids.slice(1).map(color)).toStrictEqual(["#e4e4e4", "#ffff87", "#ff8700"]);
  // No sky behind any of it.
  expect(nodes(strip).some((n) => n.calls.some(([k]) => k === "bg"))).toBe(false);

  expect(text.calls).toContainEqual(["font_family", "Menlo"]);
  // Two shadow rings, an outline of eight copies, then the time in its hour's color on top.
  const [outer, inner, ...edge] = text.kids;
  expect([outer, inner].map((ring) => arg(ring, "opacity"))).toStrictEqual([0.3, 0.6]);
  expect(outer.kids[0].calls.slice(1, 3)).toStrictEqual([
    ["top", -3],
    ["left", -4],
  ]);
  expect(edge.map(color)).toStrictEqual([...Array.from({ length: 8 }, () => "#000000"), "#bd93f9"]);
  expect(edge.map((e) => e.kids[0])).toStrictEqual(Array.from({ length: 9 }, () => "18:25:30"));
});

test("the bar starts past the sidebar, or at the window's edge without one", () => {
  expect(sidebarWidth(null)).toBe(0);
  expect(sidebarWidth({ id: "root", children: [{ id: "sidebar", size: { px: 200 } }] })).toBe(200);
  expect(sidebarWidth({ id: "root", children: [{ id: "sidebar", hidden: true }] })).toBe(0);
  expect(sidebarWidth({ id: "root", children: [{ id: "x" }, { id: "sidebar" }] })).toBe(0);
});

test("the cat runs with the window in front and creeps once a second behind, on the second", () => {
  const afters = [];
  const cx = { timer: { after: (ms, fn) => afters.push({ ms, fn }) }, notify: vi.fn() };
  host.window.active = true;
  host.layout = { id: "root", children: [{ id: "sidebar", size: { px: 268 } }] };
  const view = new ClockCat();
  view.init({}, cx);
  expect(view.look.time).toBe("18:25:30");
  // 40 ms past the second: the next 90 ms boundary is 50 ms away.
  expect(afters[0].ms).toBe(50);
  host.window.active = false;
  vi.setSystemTime(new Date(2026, 10, 14, 18, 30, 0, 250));
  afters[0].fn(cx);
  expect(view.f).toBe(1);
  expect(view.look).toMatchObject({ time: "18:30:00", fur: 0, taste: 5 });
  expect(cx.notify).toHaveBeenCalledTimes(1);
  expect(afters[1].ms).toBe(750);
  const strip = view.render();
  expect(paths(strip)).toContain("mods/tabs-clock/nyan/cat-24-1.svg");
  expect(arg(strip.kids[1], "w")).toBe(268);
  // At 18:30:00.250 the cat has just set out.
  expect(arg(strip.kids[2], "flex_grow")).toBe(250 / 60_000);
  // The frame wraps after the twelfth.
  view.f = 11;
  afters[1].fn(cx);
  expect(view.f).toBe(0);
});
