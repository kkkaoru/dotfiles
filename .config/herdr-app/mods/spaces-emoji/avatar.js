// A space's avatar, shared by spaces-emoji and agents-emoji: its stored color behind an animal
// emoji picked from its name.

import { h_flex } from "gpui-base";
import { shared } from "herdr";

// What a stored color name looks like on an avatar (the same in every mod
// that draws space avatars): one strong color per hue, light and dark alike.
export const MARK = {
  ansi6: "#22a99a",
  ansi3: "#e3814f",
  ansi4: "#5b8def",
  ansi5: "#8b7cf6",
  ansi2: "#3fae6a",
  ansi1: "#d9649a",
};

// A space's avatar: its name hashed (FNV-1a) onto one of a fixed set of animal emoji code
// points, so the same name always gets the same animal.
const EMOJI_RANGES = [
  [0x1f400, 0x1f43e],
  [0x1f980, 0x1f9ae],
];
const EMOJI = EMOJI_RANGES.flatMap(([lo, hi]) =>
  Array.from({ length: hi - lo + 1 }, (_, i) => String.fromCodePoint(lo + i)),
);
export function spaceEmoji(name) {
  let h = 0x811c9dc5;
  for (const ch of name) {
    h = Math.imul(h ^ ch.codePointAt(0), 0x01000193) >>> 0;
  }
  return EMOJI[h % EMOJI.length];
}

/** The fill a space's avatar is drawn with: its picked color, else the default teal. */
export function spaceFill(id, t) {
  const color = (shared("space.colors") ?? {})[id] ?? "ansi6";
  return color.startsWith("#") ? color : (MARK[color] ?? t[color]);
}

export const spaceAvatar = (id, name, t) =>
  h_flex()
    .size(18)
    .flex_none()
    .rounded(5)
    .bg(spaceFill(id, t))
    .items_center()
    .justify_center()
    .text_size(12)
    .child(spaceEmoji(name));
