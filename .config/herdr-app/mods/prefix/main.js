// herdr (TUI) style prefix mode: ctrl-b opens this popup, shows that the prefix
// is armed, and the next key runs one command. Esc or any other key cancels.

import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { run, props, closeOverlay, theme, focused, hc, nextCommand } from "herdr";

// Mirrors ~/.config/herdr/config.toml (focus_pane_left = prefix+left, split_horizontal = prefix+h).
const KEYS = [
  ["c", "new tab", "tab.new"],
  ["n", "next tab", "tab.next"],
  ["p", "prev tab", "tab.prev"],
  ["1-9", "select tab", null],
  ["shift-t", "rename tab", "tab.rename"],
  ["shift-x", "close tab", "tab.close"],
  ["shift-n", "new space", "space.new"],
  ["shift-w", "rename space", "space.rename"],
  ["shift-d", "close space", "space.close"],
  ["v", "split right", "pane.split", { direction: "right" }],
  ["h", "split down", "pane.split", { direction: "down" }],
  ["left", "focus left", "pane.go", { direction: "left" }],
  ["j", "focus down", "pane.go", { direction: "down" }],
  ["k", "focus up", "pane.go", { direction: "up" }],
  ["l", "focus right", "pane.go", { direction: "right" }],
  ["down", null, "pane.go", { direction: "down" }],
  ["up", null, "pane.go", { direction: "up" }],
  ["right", null, "pane.go", { direction: "right" }],
  ["x", "close pane", "pane.close"],
  ["z", "zoom", "pane.zoom"],
  ["b", "sidebar", "layout.toggle", { id: "sidebar" }],
  ["w", "picker", "mod.open", { name: "picker" }],
  ["g", "palette", "app.palette"],
  ["s", "settings", "app.settings"],
  ["shift-/", "help", "app.help"],
  ["q", "hide", "app.hide"],
];
const BY_KEY = new Map(KEYS.filter((k) => k[2]).map((k) => [k[0], k]));

const sendLiteralPrefix = () => {
  const terminal = focused()?.terminal;
  if (terminal) hc(["keys", terminal, "ctrl+b"]);
};

export default class Prefix extends View {
  init(_props, cx) {
    this.handle = cx.focus_handle();
    this.handle.focus();
    // ctrl-b while armed comes through keys.json's mod:prefix section.
    cx.spawn(async () => {
      for (;;) if ((await nextCommand()) === "literal") this.finish(sendLiteralPrefix);
    });
  }

  finish(action) {
    closeOverlay(props().overlay);
    action?.();
  }

  key(e) {
    const stroke = e.keystroke === "?" ? "shift-/" : e.keystroke;
    if (/^[1-9]$/.test(stroke)) return () => run("tab.select", { index: Number(stroke) });
    const hit = BY_KEY.get(stroke);
    return hit && (() => run(hit[2], hit[3] ?? {}));
  }

  render(_cx) {
    const t = theme();
    const label = (k) => k.replace("shift-/", "?").replace(/^shift-(.)$/, (_, c) => c.toUpperCase());
    const hint = ([k, what]) =>
      h_flex().w(130).gap(6).child(div().w(40).text_color(t.accent).child(label(k))).child(div().text_color(t.muted).child(what));
    return v_flex()
      .id("prefix")
      .track_focus(this.handle)
      .on_key_down((e, cx) => {
        if (["shift", "control", "alt", "platform", "function"].includes(e.key)) return;
        cx.stop_propagation();
        this.finish(e.key === "escape" ? null : this.key(e));
      })
      .size_full()
      .p(14)
      .gap(10)
      .rounded(8)
      .bg(t.popover)
      .text_size(12)
      .child(
        h_flex()
          .gap(8)
          .items_center()
          .child(div().px(8).py(2).rounded(5).bg(t.accent).text_color(t.accent_foreground).font_semibold().child("PREFIX"))
          .child(div().text_color(t.foreground).child("ctrl-b"))
          .child(div().flex_1())
          .child(div().text_size(11).text_color(t.dim).child("Esc to cancel · ctrl-b sends ctrl-b")),
      )
      .child(h_flex().flex_wrap().gap_y(4).children(KEYS.filter((k) => k[1]).map(hint)));
  }
}
