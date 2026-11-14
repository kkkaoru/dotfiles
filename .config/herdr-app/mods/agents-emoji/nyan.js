// The Agents list's nyan cats: frames, per-agent variants, and the view that runs one cat.

import { View, div, image, svg } from "gpui-kit";
import { h_flex } from "gpui-base";
import { window } from "herdr";

// Nyan cat frames from scripts/build-nyancat-frames.py: the cat (47x21) and a rainbow tail
// (NYAN_TAIL x 21) repeated to its left to fill the row.
const NYAN_TAIL = 128;
const NYAN_FRAMES = 12;
export const NYAN_MS = 90;
// A stopped agent's cat creeps on at 1 FPS.
const NYAN_RESTING_MS = 1000;
// Fur, and poptart frosting with its sprinkles. The first of each is the original.
const FUR = ["#585858", "#d7875f", "#e4e4e4", "#d7d7af", "#5f87af", "#875f00"]; // gray, ginger, white, cream, blue, brown
const POPTART = [
  ["#d787af", "#d70087"],
  ["#875f5f", "#ffd7af"],
  ["#87af5f", "#ffffff"],
  ["#ffff87", "#ff8700"],
  ["#87ffd7", "#ff5f87"],
  ["#ffffff", "#00afff"],
]; // strawberry, chocolate, matcha, lemon, mint, vanilla
// The night sky behind the cat (nyancat.c's ",").
const SKY = "#00005f";
// Letter color on each rainbow: black, or white on the two whose middle bands are dark (lava,
// galaxy). Chosen by WCAG contrast against the two bands the letters sit on (bands 3 and 4).
const LIGHT_INK = new Set([14, 16]);
export const nyanInk = (r) => (LIGHT_INK.has(r) ? "#ffffff" : "#000000");
// The letters' outline, the other of black and white: whatever band is behind, each letter sits
// on its own outline at 21:1, past WCAG AA's 4.5:1.
export const nyanHalo = (r) => (LIGHT_INK.has(r) ? "#000000" : "#ffffff");
// The outline is the letter stamped 1px off in each of eight directions under the ink copy.
const HALO = [
  [-1, -1],
  [0, -1],
  [1, -1],
  [-1, 0],
  [1, 0],
  [-1, 1],
  [0, 1],
  [1, 1],
];
// A white outline gets a black one 2px out around it, so it keeps its edge on light bands too.
const RIM = HALO.map(([dx, dy]) => [dx * 2, dy * 2]);
const rimOf = (r) => (LIGHT_INK.has(r) ? [] : RIM);
// Rainbow palettes baked into the frames: len(RAINBOWS) in the build script.
const NYAN_RAINBOWS = 18;
const fnv = (s) => {
  let h = 0x811c9dc5;
  for (const ch of s) {
    h = Math.imul(h ^ ch.codePointAt(0), 0x01000193) >>> 0;
  }
  return h;
};
// Each agent's fur, poptart and rainbow. An agent keeps the cat it was given, wherever its row
// moves; a new one picks each part by its key (tab name, address and program) hashed with the
// part's name, moved on to the next choice the fewest listed agents have.
const NYAN_PARTS = [
  ["fur", FUR.length],
  ["poptart", POPTART.length],
  ["rainbow", NYAN_RAINBOWS],
];
// The choice at `start`, or the next one used least (unused while any is free).
function leastUsed(used, n, start) {
  const fewest = Math.min(...Array.from({ length: n }, (_, v) => used[v] ?? 0));
  return Array.from({ length: n }, (_, i) => (start + i) % n).find(
    (v) => (used[v] ?? 0) === fewest,
  );
}

// `kept`: the previous result, whose listed agents keep their cats.
export function nyanVariants(keys, kept = new Map()) {
  const variants = new Map(
    keys.filter(([addr]) => kept.has(addr)).map(([addr]) => [addr, kept.get(addr)]),
  );
  const used = NYAN_PARTS.map(() => []);
  for (const parts of variants.values()) {
    parts.forEach((v, p) => (used[p][v] = (used[p][v] ?? 0) + 1));
  }
  for (const [addr, key] of keys) {
    if (variants.has(addr)) {
      continue;
    }
    const parts = NYAN_PARTS.map(([part, n], p) =>
      leastUsed(used[p], n, fnv(`${part}\u0000${key}`) % n),
    );
    parts.forEach((v, p) => (used[p][v] = (used[p][v] ?? 0) + 1));
    variants.set(addr, parts);
  }
  return variants;
}

// How long ago, in its largest whole unit: 45s, 12m, 3h, 2d.
const UNITS = [
  [86_400_000, "d"],
  [3_600_000, "h"],
  [60_000, "m"],
  [1000, "s"],
];
export function ago(ms) {
  const [size, unit] = UNITS.find(([size]) => ms >= size) ?? UNITS.at(-1);
  return `${Math.floor(Math.max(ms, 0) / size)}${unit}`;
}

// What the rainbow says: the row's name, and how long the agent has been in its state when known.
export const nyanLabel = ({ name, since }, now) =>
  since === undefined ? name : `${name}·${ago(now - since)}`;

// The rainbow is drawn in segments 8 cells wide that sit one cell up or down by turns, flipping
// every second frame. Each letter takes one segment and rides it, in the ink its rainbow reads
// best in, so the words wave with the tail. Counted from the cat, segment k is up when
// k + floor(f / 2) is odd (trail tiles hold an even number of segments and end at the cat).
const SEGMENT_CELLS = 8;
// Row space the label leaves to the avatar, a state icon, gaps and padding (px).
const ROW_CHROME = 96;
export const segmentUp = (k, f) => (k + Math.floor(f / 2)) % 2 === 1;

// The label in at most `room` letters: a long name loses its end, never the time after it.
export function fitLabel(text, room) {
  const letters = [...text];
  if (letters.length <= room) {
    return letters;
  }
  if (room <= 0) {
    return [];
  }
  const dot = letters.lastIndexOf("·");
  const tail = dot < 0 ? [] : letters.slice(dot);
  return [...letters.slice(0, Math.max(room - tail.length - 1, 0)), "…", ...tail].slice(-room);
}

// What a cat's letters need: they change only with the label, its room or the wave's phase.
export function lettersOf({ h, width, r, name, since }, f, now) {
  const seg = (SEGMENT_CELLS * h) / 21;
  const room = Math.max(Math.floor((width - ROW_CHROME - (h * 47) / 21) / seg), 0);
  const letters = name ? fitLabel(nyanLabel({ name, since }, now), room) : [];
  return { h, r, letters, phase: Math.floor(f / 2) % 2 };
}

// One copy of a letter over its whole segment, `dx`/`dy` px off center.
const glyph = (letter, color, size, [dx, dy]) =>
  div()
    .absolute()
    .top(dy)
    .left(dx)
    .size_full()
    .flex()
    .justify_center()
    .items_center()
    .text_color(color)
    .text_size(size)
    .font_weight(900)
    .child(letter);

export function rainbowLetters({ h, r, letters, phase }) {
  const cell = h / 21;
  const size = h < 20 ? 9 : 11;
  return h_flex()
    .absolute()
    .top_0()
    .right((h * 47) / 21)
    .h(h)
    .children(
      letters.map((letter, i) =>
        div()
          .relative()
          .w(SEGMENT_CELLS * cell)
          .h(h)
          .flex_none()
          .when(!segmentUp(letters.length - 1 - i, phase * 2), (el) => el.mt(cell))
          .children(rimOf(r).map((at) => glyph(letter, "#000000", size, at)))
          .children(HALO.map((at) => glyph(letter, nyanHalo(r), size, at)))
          .child(glyph(letter, nyanInk(r), size, [0, 0])),
      ),
    );
}

// A cat's letters as their own view: the cat redraws every frame, the letters only when the wave
// flips (every second frame) or the label changes.
export class NyanLetters extends View {
  init(p) {
    this.p = p;
  }

  update(p) {
    this.p = p;
  }

  render() {
    return rainbowLetters(this.p);
  }
}

// The next frame of this cat's rainbow, 1px and transparent, so it is loaded before it shows:
// image() draws nothing until a picture has loaded, which blinked a cat that started running.
const nextFrame = (r, f) =>
  div()
    .absolute()
    .top_0()
    .left_0()
    .size(1)
    .overflow_hidden()
    .opacity(0)
    .child(image(`mods/agents-emoji/nyan/trail-${r}-${(f + 1) % NYAN_FRAMES}.svg`).size(1))
    .child(image(`mods/agents-emoji/nyan/cat-${r}-${(f + 1) % NYAN_FRAMES}.svg`).size(1));

// One frame of a cat: its rainbow tail, the cat with its rainbow, fur and poptart over it, and
// its letters (a NyanLetters view) on the tail when it has them.
export function nyanStrip(p, f, letters) {
  const { h, fur, taste, r, width } = p;
  const [frosting, sprinkles] = POPTART[taste];
  const w = (h * 47) / 21;
  const tw = (h * NYAN_TAIL) / 21;
  const layer = (el) => el.absolute().top_0().left_0().w(w).h(h);
  return h_flex()
    .relative()
    .flex_1()
    .min_w(0)
    .h(h)
    .overflow_hidden()
    .rounded(3)
    .bg(SKY)
    .justify_end()
    .child(nextFrame(r, f))
    .children(
      Array.from({ length: Math.ceil(width / tw) }, () =>
        image(`mods/agents-emoji/nyan/trail-${r}-${f}.svg`).h(h).w(tw).flex_none(),
      ),
    )
    .child(
      div()
        .relative()
        .w(w)
        .h(h)
        .flex_none()
        .child(layer(image(`mods/agents-emoji/nyan/cat-${r}-${f}.svg`)))
        .child(layer(svg(`mods/agents-emoji/nyan/fur-${f}.svg`)).text_color(FUR[fur]))
        .child(layer(svg(`mods/agents-emoji/nyan/frosting-${f}.svg`)).text_color(frosting))
        .child(layer(svg(`mods/agents-emoji/nyan/sprinkles-${f}.svg`)).text_color(sprinkles)),
    )
    .when(Boolean(letters), (el) => el.child(letters));
}

// One agent's cat as its own view, so a frame redraws the cat and not the whole list. It runs
// while the agent works and creeps while it's stopped, going on from where it was.
export class NyanCat extends View {
  init(p, cx) {
    this.p = p;
    this.f = 0;
    this.timer = cx.timer;
    this.lettersKey = "";
    this.letters = cx.new(NyanLetters, this.nextLetters());
    this.schedule();
  }

  // The letters' props when they changed since last time, else null.
  nextLetters() {
    const next = lettersOf(this.p, this.f, Date.now());
    const key = JSON.stringify(next);
    if (key === this.lettersKey) {
      return null;
    }
    this.lettersKey = key;
    return next;
  }

  // One wake per frame, none in between: 90 ms while working with the window in front, else a second.
  // The resting second also keeps the label's elapsed time current. Every cat wakes on the same
  // clock (multiples of its period), so their frames land in one window redraw, not one per cat.
  schedule() {
    const period = this.p.working && window()?.active ? NYAN_MS : NYAN_RESTING_MS;
    this.tick?.cancel();
    this.tick = this.timer.after(period - (Date.now() % period), (cx) => {
      this.f = (this.f + 1) % NYAN_FRAMES;
      const letters = this.nextLetters();
      if (letters) {
        this.letters.set_props(letters);
      }
      cx.notify();
      this.schedule();
    });
  }

  update(p) {
    const started = p.working && !this.p.working;
    this.p = p;
    // A cat that starts running shouldn't wait out its resting second.
    if (started) {
      this.schedule();
    }
  }

  render() {
    return nyanStrip(this.p, this.f, this.letters);
  }
}
