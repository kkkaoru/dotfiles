import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

import { host, reset } from "./mocks/herdr.js";
import {
  NYAN_MS,
  NyanCat,
  NyanLetters,
  ago,
  fitLabel,
  lettersOf,
  nyanHalo,
  nyanInk,
  nyanLabel,
  nyanStrip,
  nyanVariants,
  rainbowLetters,
  segmentUp,
} from "../../.config/herdr-app/mods/agents-emoji/nyan.js";

const APP_ROOT = fileURLToPath(new URL("../../.config/herdr-app/", import.meta.url));

// A timer whose waits a test fires by hand.
function fakeTimer() {
  const waits = [];
  return {
    waits,
    after: (ms, fn) => {
      const wait = { ms, fn, cancel: vi.fn() };
      waits.push(wait);
      return wait;
    },
  };
}

function newCat(p) {
  const timer = fakeTimer();
  const cat = new NyanCat();
  const cx = {
    timer,
    new: (Cls, props) => {
      const view = new Cls();
      view.init(props, cx);
      return view;
    },
  };
  cat.init(p, cx);
  return { cat, timer };
}

const fire = (timer) => {
  const cx = { notify: vi.fn() };
  timer.waits.at(-1).fn(cx);
  return cx;
};

// On the shared clock's beat, so a cat waits its whole period.
beforeEach(() => {
  vi.spyOn(Date, "now").mockReturnValue(0);
});

afterEach(() => {
  vi.restoreAllMocks();
  reset();
});

// Pinned from the code as it was before nyanVariants was restructured: a change recolors everyone's cats.
test("the variants for a fixed list stay as they were", () => {
  const keys = Array.from({ length: 9 }, (_, i) => [
    `t${i}`,
    [`tab${i % 3}`, `w${i}:p${i}`, i % 2 ? "claude" : "codex", i, i % 4].join("\u0000"),
  ]);
  keys.push(["dup", keys[0][1]]);
  expect([...nyanVariants(keys).values()]).toStrictEqual([
    [3, 4, 0],
    [1, 0, 12],
    [5, 5, 8],
    [4, 3, 11],
    [0, 1, 1],
    [2, 2, 17],
    [5, 0, 4],
    [1, 4, 10],
    [0, 5, 13],
    [3, 1, 2],
  ]);
});

test("three agents get three different furs, poptarts and rainbows", () => {
  const v = [
    ...nyanVariants([
      ["t1", "a\u0000t1\u00000"],
      ["t2", "a\u0000t2\u00000"],
      ["t3", "b\u0000t3\u00001"],
    ]).values(),
  ];
  expect(new Set(v.map((p) => p[0])).size).toBe(3);
  expect(new Set(v.map((p) => p[1])).size).toBe(3);
  expect(new Set(v.map((p) => p[2])).size).toBe(3);
});

test("agents with the same key still get different cats", () => {
  const v = [
    ...nyanVariants([
      ["a", "same"],
      ["b", "same"],
    ]).values(),
  ];
  expect(v[0][0] === v[1][0]).toBe(false);
  expect(v[0][2] === v[1][2]).toBe(false);
});

test("the same key picks the same cat every time", () => {
  expect([...nyanVariants([["a", "key"]]).values()]).toStrictEqual([
    ...nyanVariants([["a", "key"]]).values(),
  ]);
});

test("past six agents the furs repeat, all six used first", () => {
  const keys = Array.from({ length: 7 }, (_, i) => [`t${i}`, `k${i}`]);
  const furs = [...nyanVariants(keys).values()].map((p) => p[0]);
  expect(new Set(furs.slice(0, 6)).size).toBe(6);
  expect(furs.every((f) => f >= 0 && f < 6)).toBe(true);
});

test("each variant names a fur, a poptart and a rainbow in range", () => {
  const [fur, taste, r] = nyanVariants([["a", "x"]]).get("a");
  expect(fur >= 0 && fur < 6).toBe(true);
  expect(taste >= 0 && taste < 6).toBe(true);
  expect(r >= 0 && r < 18).toBe(true);
});

test("a strip loads its rainbow's frames, repeats the tail to fill the row, then draws the cat", () => {
  const strip = nyanStrip({ h: 21, fur: 1, taste: 2, r: 4, width: 300 }, 7, null);
  expect(strip.kind).toBe("h_flex");
  expect(strip.kids.slice(1, 4).map((c) => c.path)).toStrictEqual([
    "mods/agents-emoji/nyan/trail-4-7.svg",
    "mods/agents-emoji/nyan/trail-4-7.svg",
    "mods/agents-emoji/nyan/trail-4-7.svg",
  ]);
  expect(strip.kids.length).toBe(5);
  const cat = strip.kids[4];
  expect(cat.kids.map((c) => c.path)).toStrictEqual([
    "mods/agents-emoji/nyan/cat-4-7.svg",
    "mods/agents-emoji/nyan/fur-7.svg",
    "mods/agents-emoji/nyan/frosting-7.svg",
    "mods/agents-emoji/nyan/sprinkles-7.svg",
  ]);
  expect(
    cat.kids.slice(1).map((layer) => layer.calls.find(([k]) => k === "text_color")[1]),
  ).toStrictEqual(["#d7875f", "#87af5f", "#ffffff"]);
  expect(cat.calls.slice(0, 3)).toStrictEqual([["relative"], ["w", 47], ["h", 21]]);
});

test("a narrow row still gets one tail", () => {
  expect(nyanStrip({ h: 21, fur: 0, taste: 0, r: 0, width: 1 }, 0, null).kids.length).toBe(3);
});

test("a strip keeps its next frame loaded, transparent, and the files exist", () => {
  const ahead = nyanStrip({ h: 21, fur: 0, taste: 0, r: 17, width: 1 }, 11, null).kids[0];
  const paths = ahead.kids.map((c) => c.path);
  expect(paths).toStrictEqual([
    "mods/agents-emoji/nyan/trail-17-0.svg",
    "mods/agents-emoji/nyan/cat-17-0.svg",
  ]);
  expect(paths.filter((path) => !existsSync(`${APP_ROOT}${path}`))).toStrictEqual([]);
  expect(ahead.calls).toStrictEqual([
    ["absolute"],
    ["top_0"],
    ["left_0"],
    ["size", 1],
    ["overflow_hidden"],
    ["opacity", 0],
  ]);
});

test("every cat wakes on the same beat, however late it started", () => {
  Date.now.mockReturnValue(1030);
  expect(newCat({ working: true }).timer.waits[0].ms).toBe(50);
  expect(newCat({ working: false }).timer.waits[0].ms).toBe(970);
});

test("a stopped agent's cat waits a second per frame", () => {
  const { timer } = newCat({ working: false });
  expect(timer.waits.map((w) => w.ms)).toStrictEqual([1000]);
});

test("a working agent's cat runs at 90 ms with the window in front", () => {
  const { timer } = newCat({ working: true });
  expect(timer.waits.map((w) => w.ms)).toStrictEqual([NYAN_MS]);
  expect(NYAN_MS).toBe(90);
});

test("a working agent's cat slows to a second behind other apps", () => {
  host.window = { active: false };
  expect(newCat({ working: true }).timer.waits.map((w) => w.ms)).toStrictEqual([1000]);
});

test("a cat with no window rests", () => {
  host.window = null;
  expect(newCat({ working: true }).timer.waits.map((w) => w.ms)).toStrictEqual([1000]);
});

test("each tick moves one frame, redraws and waits again", () => {
  const { cat, timer } = newCat({ working: true });
  const cx = fire(timer);
  expect(cat.f).toBe(1);
  expect(cx.notify).toHaveBeenCalledTimes(1);
  expect(timer.waits.length).toBe(2);
  expect(timer.waits[0].cancel).toHaveBeenCalledTimes(1);
});

test("the frames wrap after twelve", () => {
  const { cat, timer } = newCat({ working: true });
  Array.from({ length: 12 }, () => fire(timer));
  expect(cat.f).toBe(0);
});

test("a cat that starts running stops waiting out its resting second", () => {
  const { cat, timer } = newCat({ working: false });
  cat.update({ working: true });
  expect(timer.waits.map((w) => w.ms)).toStrictEqual([1000, 90]);
  expect(timer.waits[0].cancel).toHaveBeenCalledTimes(1);
});

test("new props without a start keep the wait", () => {
  const { cat, timer } = newCat({ working: true });
  cat.update({ working: true, fur: 2 });
  cat.update({ working: false });
  expect(timer.waits.length).toBe(1);
  expect(cat.p).toStrictEqual({ working: false });
});

test("a cat draws its current frame", () => {
  const { cat, timer } = newCat({ h: 21, fur: 0, taste: 0, r: 3, width: 100, working: true });
  fire(timer);
  expect(cat.render().kids[1].path).toBe("mods/agents-emoji/nyan/trail-3-1.svg");
});

test("elapsed time reads in its largest whole unit", () => {
  expect(ago(0)).toBe("0s");
  expect(ago(-5000)).toBe("0s");
  expect(ago(45_999)).toBe("45s");
  expect(ago(60_000)).toBe("1m");
  expect(ago(3 * 3_600_000 + 59 * 60_000)).toBe("3h");
  expect(ago(2 * 86_400_000)).toBe("2d");
});

test("a label is the name, with the time in its state when known", () => {
  expect(nyanLabel({ name: "dotfiles", since: 1000 }, 721_000)).toBe("dotfiles·12m");
  expect(nyanLabel({ name: "dotfiles" }, 721_000)).toBe("dotfiles");
});

test("a long label loses the end of its name, never its time", () => {
  expect(fitLabel("dotfiles·12m", 12)).toStrictEqual([..."dotfiles·12m"]);
  expect(fitLabel("tiktok-live·5m", 9)).toStrictEqual([..."tikto…·5m"]);
  expect(fitLabel("a-long-name", 4)).toStrictEqual([..."a-l…"]);
  expect(fitLabel("x·12m", 2)).toStrictEqual([..."2m"]);
  expect(fitLabel("x", 0)).toStrictEqual([]);
});

test("the rainbow's segments rise and fall by turns, flipping every second frame", () => {
  expect([0, 1, 2, 3].map((k) => segmentUp(k, 0))).toStrictEqual([false, true, false, true]);
  expect([0, 1].map((k) => segmentUp(k, 1))).toStrictEqual([false, true]);
  expect([0, 1].map((k) => segmentUp(k, 2))).toStrictEqual([true, false]);
});

test("each letter rides one segment, ending at the cat, lowered on the segments that are down", () => {
  const letters = rainbowLetters(
    lettersOf({ h: 21, r: 0, width: 300, name: "ab", since: 0 }, 0, 0),
  );
  expect(letters.calls.slice(0, 4)).toStrictEqual([
    ["absolute"],
    ["top_0"],
    ["right", 47],
    ["h", 21],
  ]);
  // Each segment holds its letter 17 times: a black rim 2px out, the white outline 1px out, then
  // the ink on top.
  expect(letters.kids.map((l) => l.kids.map((g) => g.kids[0]))).toStrictEqual(
    [..."ab·0s"].map((letter) => Array.from({ length: 17 }, () => letter)),
  );
  expect(
    letters.kids[0].kids.map((g) => g.calls.find(([k]) => k === "text_color")[1]),
  ).toStrictEqual([
    ...Array.from({ length: 8 }, () => "#000000"),
    ...Array.from({ length: 8 }, () => "#ffffff"),
    "#000000",
  ]);
  expect(letters.kids[0].kids.slice(0, 2).map((g) => g.calls.slice(0, 3))).toStrictEqual([
    [["absolute"], ["top", -2], ["left", -2]],
    [["absolute"], ["top", -2], ["left", 0]],
  ]);
  // From the cat: s is segment 0 (down), 0 is 1 (up), and so on.
  expect(letters.kids.map((l) => l.calls.some(([k]) => k === "mt"))).toStrictEqual([
    true,
    false,
    true,
    false,
    true,
  ]);
  expect(letters.kids[0].calls.find(([k]) => k === "w")).toStrictEqual(["w", 8]);
  const pick = (g) =>
    g.calls.filter(([k]) => ["top", "left", "text_color", "text_size", "font_weight"].includes(k));
  expect(pick(letters.kids[0].kids[8])).toStrictEqual([
    ["top", -1],
    ["left", -1],
    ["text_color", "#ffffff"],
    ["text_size", 11],
    ["font_weight", 900],
  ]);
  expect(
    letters.kids[0].kids.slice(8, 16).map((g) =>
      pick(g)
        .slice(0, 2)
        .map(([, v]) => v),
    ),
  ).toStrictEqual([
    [-1, -1],
    [-1, 0],
    [-1, 1],
    [0, -1],
    [0, 1],
    [1, -1],
    [1, 0],
    [1, 1],
  ]);
  expect(pick(letters.kids[0].kids[16])).toStrictEqual([
    ["top", 0],
    ["left", 0],
    ["text_color", "#000000"],
    ["text_size", 11],
    ["font_weight", 900],
  ]);
});

test("a black outline on a dark rainbow gets no rim", () => {
  const lava = rainbowLetters(lettersOf({ h: 21, r: 14, width: 300, name: "a" }, 0, 0));
  expect(lava.kids[0].kids.map((g) => g.calls.find(([k]) => k === "text_color")[1])).toStrictEqual([
    ...Array.from({ length: 8 }, () => "#000000"),
    "#ffffff",
  ]);
});

test("a small cat's letters are smaller, and a row too narrow for any gets none", () => {
  const small = rainbowLetters(lettersOf({ h: 18, r: 0, width: 300, name: "k" }, 0, 0));
  expect(small.kids[0].kids[16].calls.find(([k]) => k === "text_size")).toStrictEqual([
    "text_size",
    9,
  ]);
  expect(lettersOf({ h: 18, r: 0, width: 20, name: "k" }, 0, 0).letters).toStrictEqual([]);
});

test("a cat without a name has no letters", () => {
  expect(lettersOf({ h: 21, r: 0, width: 300 }, 0, 0).letters).toStrictEqual([]);
});

test("a strip draws its letters view last", () => {
  const letters = { view: "letters" };
  expect(nyanStrip({ h: 21, fur: 0, taste: 0, r: 0, width: 1 }, 0, letters).kids.at(-1)).toBe(
    letters,
  );
});

test("a cat's letters redraw only when the wave flips or the label changes", () => {
  const { cat, timer } = newCat({
    h: 21,
    fur: 0,
    taste: 0,
    r: 0,
    width: 300,
    name: "ab",
    since: 0,
    working: true,
  });
  expect(cat.letters).toBeInstanceOf(NyanLetters);
  expect(cat.letters.render().kids.map((l) => l.kids[16].kids[0])).toStrictEqual([..."ab·0s"]);
  const set = vi.spyOn(cat.letters, "set_props");
  fire(timer);
  expect(set).not.toHaveBeenCalled();
  fire(timer);
  expect(set).toHaveBeenCalledTimes(1);
  expect(cat.letters.p.phase).toBe(1);
  Date.now.mockReturnValue(5000);
  fire(timer);
  expect(cat.letters.p.letters.join("")).toBe("ab·5s");
});

// The build script's palettes decide the ink: black unless white reads better on both middle bands.
test("each rainbow's ink is the one with more contrast on the bands its letters sit on", () => {
  const script = readFileSync(
    new URL("../../scripts/build-nyancat-frames.py", import.meta.url),
    "utf8",
  );
  const block = script.slice(
    script.indexOf("RAINBOWS = ["),
    script.indexOf("]\n", script.indexOf("RAINBOWS = [")),
  );
  const palettes = block
    .split("\n")
    .filter((line) => line.trim().startsWith("("))
    .map((line) => line.match(/#[0-9a-f]{6}/g));
  const channel = (hex, i) => {
    const c = Number.parseInt(hex.slice(1 + i * 2, 3 + i * 2), 16) / 255;
    return c <= 0.039_28 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  const luminance = (hex) =>
    0.2126 * channel(hex, 0) + 0.7152 * channel(hex, 1) + 0.0722 * channel(hex, 2);
  const contrast = (a, b) =>
    (Math.max(luminance(a), luminance(b)) + 0.05) / (Math.min(luminance(a), luminance(b)) + 0.05);
  const best = (bands) =>
    Math.min(...bands.map((band) => contrast("#ffffff", band))) >
    Math.min(...bands.map((band) => contrast("#000000", band)))
      ? "#ffffff"
      : "#000000";
  expect(palettes.length).toBe(18);
  expect(palettes.map((_, r) => nyanInk(r))).toStrictEqual(
    palettes.map((bands) => best(bands.slice(2, 4))),
  );
  // The outline guarantees WCAG AA (4.5:1) for the letters on every rainbow.
  expect(palettes.map((_, r) => contrast(nyanInk(r), nyanHalo(r)) >= 4.5)).toStrictEqual(
    palettes.map(() => true),
  );
});

test("agents keep their cats when their rows move, and a new one takes the least used", () => {
  const keys = Array.from({ length: 7 }, (_, i) => [`a${i}`, `tab\u0000a${i}\u0000claude`]);
  const before = nyanVariants(keys);
  const after = nyanVariants([...keys].reverse(), before);
  expect(keys.map(([addr]) => after.get(addr))).toStrictEqual(
    keys.map(([addr]) => before.get(addr)),
  );
  const added = nyanVariants([["new", "tab\u0000new\u0000codex"], ...keys.slice(0, 2)], before);
  expect(
    added.get("new").map((v, p) => [before.get("a0")[p], before.get("a1")[p]].includes(v)),
  ).toStrictEqual([false, false, false]);
  expect(added.has("a6")).toBe(false);
});
