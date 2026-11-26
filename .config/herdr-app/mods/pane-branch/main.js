// A pane's header, for layouts that frame their panes:
//   { "tab": "active", "pane": { "header": { "content": { "mod": "pane-branch" } } } }
// Shows the program's icon (the app's program table), the agent's name or
// the terminal's title, the folder and the checkout's branch, and the agent's status; buttons
// split, zoom and close this pane. Props from the app: { pane, terminal }. Click: focus the pane.

import { View, div, svg, image } from "gpui-kit";
import { h_flex } from "gpui-base";
import { panes, props, run, theme, home, layout } from "herdr";

// A stack shows one child at a time, so the walk below must not reach the others.
function stackChild(node, children) {
  const visible = children.filter((child) => !child.hidden);
  return [visible.find((child) => child.id === node.active) ?? visible[0]];
}

// Match the first visible tab frame, including the active child of a stack.
function paneFrame(node) {
  if (!node || node.hidden) {
    return null;
  }
  if (node.content?.tab && node.content.pane) {
    return node.content.pane;
  }
  let children = node.children ?? [];
  if (node.type === "stack") {
    children = stackChild(node, children);
  }
  for (const child of children) {
    const frame = paneFrame(child);
    if (frame) {
      return frame;
    }
  }
  return null;
}

function icon(p, size, color) {
  if (!p?.icon) {
    return null;
  }
  return p.tint
    ? svg(p.icon).size(size).flex_none().text_color(color)
    : image(p.icon).w(size).h(size).flex_none();
}

function folder(cwd) {
  if (!cwd) {
    return "";
  }
  const h = home();
  if (cwd === h) {
    return "~";
  }
  return cwd.startsWith(h + "/") ? "~" + cwd.slice(h.length) : cwd;
}

// The folder, then the branch a split or a worktree would otherwise hide.
function place(cwd, branch) {
  const f = folder(cwd);
  return branch ? `${f} (${branch})` : f;
}

// What the pane is running: the agent's name, else the title, else the program.
function label(p) {
  if (p.name) {
    return p.name;
  }
  if ((p.program?.agent || p.program?.shell) && p.program.name) {
    return p.program.name;
  }
  return p.title || p.program?.name || "shell";
}

function status(t, st) {
  if (st === "working") {
    return svg("mods/workspace-chrome/spinner.svg")
      .size(12)
      .flex_none()
      .text_color(t.working)
      .spin(900);
  }
  if (st === "blocked") {
    return div()
      .flex_none()
      .px(6)
      .h(18)
      .flex()
      .items_center()
      .rounded(5)
      .bg(t.blocked + "26")
      .text_size(10.5)
      .font_semibold()
      .text_color(t.blocked)
      .child("Needs you");
  }
  if (st === "done") {
    return div().size(7).flex_none().rounded(4).bg(t.done);
  }
  return null;
}

function addButtons(el, view, t, p, pane) {
  return el
    .child(
      view.button(t, `ph-r-${pane}`, "split-right", "pane.split", { direction: "right", pane }),
    )
    .child(view.button(t, `ph-d-${pane}`, "split-down", "pane.split", { direction: "down", pane }))
    .child(view.button(t, `ph-z-${pane}`, p.zoomed ? "unzoom" : "zoom", "pane.zoom", { pane }))
    .child(view.button(t, `ph-x-${pane}`, "close", "pane.close", { pane }));
}

export default class Pane extends View {
  init() {
    this.hovered = false;
  }

  me() {
    return panes().find((p) => p.pane === props()?.pane);
  }

  button(t, id, icon, command, args) {
    return div()
      .id(id)
      .p(3)
      .rounded(4)
      .text_color(t.dim)
      .hover((s) => s.bg(t.hover).text_color(t.foreground))
      .on_click((_e, cx) => {
        cx.stop_propagation();
        run(command, args);
      })
      .child(svg(`mods/pane-branch/${icon}.svg`).size(14));
  }

  render(_cx) {
    const t = theme();
    const p = this.me();
    if (!p) {
      return div();
    }
    const pic = icon(p.program, 14, t.dim);
    const chip = status(t, p.status);
    const pane = p.pane;
    const bottom = paneFrame(layout())?.header?.position === "bottom";
    return h_flex()
      .id(`pane-head-${pane}`)
      .size_full()
      .px(10)
      .gap(8)
      .items_center()
      .bg(t.background)
      .when(bottom, (el) => el.border_t(1))
      .when(!bottom, (el) => el.border_b(1))
      .border_color(t.border)
      .text_size(12)
      .on_hover((hovered, cx) => {
        this.hovered = hovered;
        cx.notify();
      })
      .on_click(() => run("pane.focus", { pane }))
      .when(Boolean(pic), (el) => el.child(pic))
      .child(
        div()
          .flex_none()
          .truncate()
          .max_w(260)
          .font_medium()
          .text_color(t.foreground)
          .child(label(p)),
      )
      .child(div().flex_1().truncate().text_color(t.dim).child(place(p.cwd, p.branch)))
      .when(Boolean(chip), (el) => el.child(chip))
      .when(this.hovered, (el) => addButtons(el, this, t, p, pane));
  }
}
