// The tab bar's clock: the time, to the second, centered on the window, and a nyan cat that crosses
// the bar once a minute, its rainbow trailing from the bar's left edge. Each hour has its own
// rainbow and time color; each minute a new cat (fur and poptart) sets out. It is its own view, so a
// frame of the cat redraws the clock and not the whole bar.

import { View, div, image, svg } from "gpui-kit";
import { h_flex } from "gpui-base";
import { layout, window } from "herdr";

// Frames from scripts/build-nyancat-frames.py, without the night sky, so the cat runs over the bar
// itself. One mod can't import another's code, so the numbers and colors below repeat
// agents-emoji/nyan.js.
const NYAN = "mods/tabs-clock/nyan";
const NYAN_FRAMES = 12;
const NYAN_TAIL = 128;
// Running with the window in front; a background window's cat creeps once a second, on the second.
const NYAN_MS = 90;
const RESTING_MS = 1000;
const FUR = ["#585858", "#d7875f", "#e4e4e4", "#d7d7af", "#5f87af", "#875f00"]; // gray, ginger, white, cream, blue, brown
const POPTART = [
  ["#d787af", "#d70087"],
  ["#875f5f", "#ffd7af"],
  ["#87af5f", "#ffffff"],
  ["#ffff87", "#ff8700"],
  ["#87ffd7", "#ff5f87"],
  ["#ffffff", "#00afff"],
]; // strawberry, chocolate, matcha, lemon, mint, vanilla

// The rainbow for each hour, 0–23, by its place in the build script's list: 24 of its 28, each
// once, ordered so neighbors never look alike (galaxy at midnight, fire at dawn, sunset at five,
// lava at nine). Left out as dull: mono, coffee, sakura and ice.
export const HOUR_RAINBOWS = [
  16, 19, 4, 8, 17, 20, 6, 27, 0, 25, 9, 21, 7, 1, 10, 15, 18, 3, 24, 26, 2, 14, 22, 23,
];
export const DULL_RAINBOWS = [5, 11, 12, 13];

// The time's color for each hour: the Dracula theme's, ordered so neighbors never look alike.
export const DRACULA = [
  "#ff79c6", // pink
  "#50fa7b", // green
  "#ffb86c", // orange
  "#8be9fd", // cyan
  "#ff5555", // red
  "#f1fa8c", // yellow
  "#bd93f9", // purple
  "#f8f8f2", // foreground
];

// The strip fills the 42 px bar; the cat is 47x21 cells scaled to it, 2 px a cell.
const H = 42;
const CAT_W = (H * 47) / 21;
const TAIL_W = (H * NYAN_TAIL) / 21;
// Rainbow tiles behind the cat: enough for a bar 4096 px wide.
const TILES = 16;

export function find(node, id) {
  if (node?.id === id) {
    return node;
  }
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) {
      return found;
    }
  }
  return null;
}

// How far the sidebar pushes the bar from the window's left edge.
export function sidebarWidth(root) {
  const sidebar = find(root, "sidebar");
  return sidebar?.hidden ? 0 : (sidebar?.size?.px ?? 0);
}

const pad = (n) => String(n).padStart(2, "0");

/** `17:05:09` for a moment in local time. */
export function clockText(now) {
  return `${pad(now.getHours())}:${pad(now.getMinutes())}:${pad(now.getSeconds())}`;
}

/** What the strip shows at a moment: the time, the hour's rainbow and the minute's cat. The fur
 *  steps on one a minute and the poptart one more each six minutes, so the next cat differs in
 *  both, and minutes 0–35 show all 36 cats once. */
export function clockLook(now) {
  const minute = now.getMinutes();
  // How far through the minute, 0 at :00 to just under 1 before the next.
  const progress = (now.getSeconds() * 1000 + now.getMilliseconds()) / 60_000;
  const hour = now.getHours();
  return {
    time: clockText(now),
    r: HOUR_RAINBOWS[hour],
    ink: DRACULA[hour % DRACULA.length],
    fur: minute % FUR.length,
    taste: (minute + Math.floor(minute / FUR.length)) % POPTART.length,
    progress,
  };
}

// The time in the cat labels' style: its ink on a black outline over a soft black shadow (rings 3
// and 4 px out, fainter outside, dropped 1 px), so it reads on every rainbow.
const AROUND = [
  [-1, -1],
  [0, -1],
  [1, -1],
  [-1, 0],
  [1, 0],
  [-1, 1],
  [0, 1],
  [1, 1],
];
const SHADOW = [
  [4, 0.3],
  [3, 0.6],
];
const stamp = (time, color, [dx, dy]) =>
  div()
    .absolute()
    .top(dy)
    .left(dx)
    .size_full()
    .flex()
    .items_center()
    .justify_center()
    .text_color(color)
    .child(time);

function timeText(time, ink) {
  return (
    div()
      .absolute()
      .top_0()
      .left_0()
      .size_full()
      // Monospaced, so the width holds as the digits change; macOS's own, with a real bold.
      .font_family("Menlo")
      .font_bold()
      .text_size(26)
      .children(
        SHADOW.map(([radius, alpha]) =>
          div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .opacity(alpha)
            .children(
              AROUND.map(([dx, dy]) => stamp(time, "#000000", [dx * radius, dy * radius + 1])),
            ),
        ),
      )
      .children(AROUND.map(([dx, dy]) => stamp(time, "#000000", [dx * 2, dy * 2])))
      .child(stamp(time, ink, [0, 0]))
  );
}

const tile = (r, f) => image(`${NYAN}/trail-${r}-${f}.svg`).h(H).w(TAIL_W).flex_none();

// Every frame of this rainbow, 1px and transparent, so each is loaded before it shows: image()
// draws nothing until a picture has loaded, which blinked the cat and its rainbow.
const preload = (r) =>
  div()
    .absolute()
    .top_0()
    .left_0()
    .size(1)
    .overflow_hidden()
    .opacity(0)
    .children(
      Array.from({ length: NYAN_FRAMES }, (_v, f) => [
        image(`${NYAN}/trail-${r}-${f}.svg`).size(1),
        image(`${NYAN}/cat-${r}-${f}.svg`).size(1),
      ]).flat(),
    );

function cat({ r, fur, taste }, f) {
  const [frosting, sprinkles] = POPTART[taste];
  const layer = (el) => el.absolute().top_0().left_0().w(CAT_W).h(H);
  return div()
    .relative()
    .w(CAT_W)
    .h(H)
    .flex_none()
    .child(layer(image(`${NYAN}/cat-${r}-${f}.svg`)))
    .child(layer(svg(`${NYAN}/fur-${f}.svg`)).text_color(FUR[fur]))
    .child(layer(svg(`${NYAN}/frosting-${f}.svg`)).text_color(frosting))
    .child(layer(svg(`${NYAN}/sprinkles-${f}.svg`)).text_color(sprinkles));
}

/** One frame of the clock across the window: from `left` (the bar's left edge, past the sidebar)
 *  the rainbow, then the cat, `progress` of the way along the bar: at its left end at :00, its
 *  right end as the minute ends. The time is over the window's middle. */
export function clockStrip(look, f, left) {
  const { time, r, ink, progress } = look;
  return (
    h_flex()
      .relative()
      .size_full()
      .items_center()
      .child(preload(r))
      .child(div().w(left).flex_none())
      // The bar's width less the cat's, shared between the rainbow and the room ahead of the cat.
      .child(
        h_flex()
          .flex_basis(0)
          .flex_grow(progress)
          .min_w(0)
          .h(H)
          .overflow_hidden()
          .justify_end()
          .children(Array.from({ length: TILES }, () => tile(r, f))),
      )
      .child(cat(look, f))
      .child(
        div()
          .flex_basis(0)
          .flex_grow(1 - progress)
          .min_w(0),
      )
      .child(timeText(time, ink))
  );
}

export class ClockCat extends View {
  init(_props, cx) {
    this.f = 0;
    this.timer = cx.timer;
    this.look = clockLook(new Date());
    this.schedule();
  }

  // One wake per frame, aligned to its period so a resting cat's second lands on the clock's.
  schedule() {
    const period = window()?.active ? NYAN_MS : RESTING_MS;
    this.timer.after(period - (Date.now() % period), (cx) => {
      this.f = (this.f + 1) % NYAN_FRAMES;
      this.look = clockLook(new Date());
      cx.notify();
      this.schedule();
    });
  }

  // The bar starts past the sidebar; read here, since a frame redraws at least once a second.
  render() {
    return clockStrip(this.look, this.f, sidebarWidth(layout()));
  }
}
