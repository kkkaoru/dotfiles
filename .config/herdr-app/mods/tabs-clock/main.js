// Herdr's tab bar (cloned from the shipped `tabs`) with a nyan cat clock (clock.js) in its middle.

import { View, div, svg, image } from "gpui-kit";
import { h_flex, ScrollHandle } from "gpui-base";
import { ClockCat, find, sidebarWidth } from "./clock.js";
import { spaces, panes, run, openOverlay, theme, window, shared, layout, settings } from "herdr";

const stop = (_e, cx) => cx.stop_propagation();

// Old saved rails and a dragged-narrow sidebar can be smaller than the window buttons.
function lightsPadding(root, w) {
  if (
    !(w?.platform === "macos" && w.titlebar === "none" && !w.fullscreen && root?.traffic_lights)
  ) {
    return 10;
  }
  return Math.max(10, (root.traffic_lights[0] + 64) / settings().zoom - sidebarWidth(root));
}

function showPanel(id) {
  run("layout.select", { stack: "panel-body", child: id });
  run("layout.show", { id: "panel" });
}

// A tab's program: its agent's, else its first pane's.
function tabProgram(id) {
  const ps = panes();
  return (
    ps.find((p) => p.tab === id && p.program?.agent)?.program ??
    ps.find((p) => p.tab === id)?.program
  );
}

const programIcon = (program, t) =>
  program.tint ? svg(program.icon).size(14).text_color(t.muted) : image(program.icon).size(14);

export default class Tabs extends View {
  init(_props, cx) {
    this.pressed = null;
    this.scroll = ScrollHandle.new();
    this.panels = JSON.parse(localStorage.getItem("panels") ?? "{}");
    this.clockCat = cx.new(ClockCat, {});
  }

  // The side panel is remembered per space: switching shows that space's choice
  // (closed if it never had one), and opening or closing it records it.
  syncPanel(space, panel, body, enabled) {
    if (!space || !panel) {
      return;
    }
    const now = panel.hidden ? null : (body?.active ?? null);
    if (space.id !== this.space && this.restore(space, now, enabled)) {
      return;
    }
    if (this.waiting(now) || this.panels[space.id] === now) {
      return;
    }
    this.panels[space.id] = now;
    localStorage.setItem("panels", JSON.stringify(this.panels));
  }

  // Until the restore lands (or something else won), the old state isn't a choice.
  waiting(now) {
    if (!this.pending) {
      return false;
    }
    if (this.pending.want !== now && Date.now() < this.pending.until) {
      return true;
    }
    this.pending = null;
    return false;
  }

  // A switch to `space`: shows its stored panel; true when that is now pending.
  restore(space, now, enabled) {
    const first = this.space === undefined;
    this.space = space.id;
    this.pending = null;
    if (first && !(space.id in this.panels)) {
      return false;
    }
    const stored = this.panels[space.id] ?? null;
    const want = enabled.includes(stored) ? stored : null;
    if (want === now) {
      return false;
    }
    this.pending = { want, until: Date.now() + 1000 };
    if (want) {
      showPanel(want);
    } else {
      run("layout.hide", { id: "panel" });
    }
    return true;
  }

  // Keeps the selected tab in view when the selection moves.
  follow(shown, active) {
    const selected = shown[active]?.id;
    if (selected !== this.selected || active !== this.selectedIndex) {
      this.selected = selected;
      this.selectedIndex = active;
      if (active >= 0) {
        this.scroll.scroll_to_item(active);
      }
    }
  }

  button(id, label, action, t) {
    return div()
      .id(id)
      .size(26)
      .flex_none()
      .flex()
      .items_center()
      .justify_center()
      .rounded(6)
      .text_color(t.dim)
      .hover((s) => s.bg(t.hover).text_color(t.foreground))
      .on_mouse_down("left", stop)
      .on_click(action)
      .child(label);
  }

  tabItem({ c, i, active, shown, space }, t) {
    const program = tabProgram(c.id);
    const name = /^tb\d+$/.test(c.name) ? (program?.name ?? "Terminal") : c.name;
    return (
      h_flex()
        .id(`tab-${c.id}`)
        .h(29)
        .px(10)
        .gap(7)
        .rounded(7)
        .items_center()
        .flex_none()
        .text_size(12.8)
        // A rename or status dot on this or an earlier tab can push the active one out.
        .when(i <= active, (el) =>
          el.on_size_change((_size, cx) => {
            this.scroll.scroll_to_item(active);
            cx.notify();
          }),
        )
        .border(1)
        .border_color(c.active ? t.border : t.chrome)
        .bg(c.active ? t.raised : t.chrome)
        .text_color(c.active ? t.foreground : t.muted)
        .hover((s) => s.bg(t.hover))
        .on_mouse_down("left", (e, cx) => {
          cx.stop_propagation();
          this.pressed = i;
          if (e.click_count === 2) {
            run("tab.rename", { id: c.id });
          }
        })
        .on_mouse_up("left", () => {
          const from = this.pressed;
          this.pressed = null;
          if (from !== null && from !== i) {
            const index = space.tabs.findIndex((x) => x.id === c.id) + 1;
            run("tab.move", { id: shown[from].id, index });
          } else {
            run("tab.select", { id: c.id });
          }
        })
        .on_mouse_down("right", (_e, cx) => {
          cx.stop_propagation();
          openOverlay({
            mod: "menu",
            kind: "popup",
            width: 200,
            props: { tab: "tab", target: { id: c.id, space: space.id } },
          });
        })
        .when(Boolean(program?.icon), (el) => el.child(programIcon(program, t)))
        .child(
          div()
            .max_w(170)
            .truncate()
            .child(c.zoomed ? `${name} Z` : name),
        )
        .when(c.status === "working", (el) =>
          el.child(
            svg("mods/workspace-chrome/spinner.svg")
              .size(12)
              .flex_none()
              .text_color(t.working)
              .spin(900),
          ),
        )
        .when(c.status === "blocked" || c.status === "done", (el) =>
          el.child(div().size(6).rounded(3).bg(t[c.status])),
        )
        .when(c.active, (el) => el.child(this.closeButton(c, t)))
    );
  }

  closeButton(c, t) {
    return div()
      .id(`close-${c.id}`)
      .text_color(t.dim)
      .on_mouse_down("left", stop)
      .on_click((_e, cx) => {
        cx.stop_propagation();
        run("tab.close", { id: c.id });
      })
      .child(svg("mods/pane/close.svg").size(12));
  }

  panelButton({ id, label, icon, panel, body }, t) {
    const on = Boolean(panel) && !panel.hidden && body?.active === id;
    return h_flex()
      .id(`panel-${id}`)
      .h(26)
      .px(8)
      .gap(5)
      .flex_none()
      .items_center()
      .rounded(6)
      .text_size(12)
      .bg(on ? t.selected : t.chrome)
      .text_color(on ? t.foreground : t.muted)
      .hover((s) => s.bg(t.hover))
      .on_mouse_down("left", stop)
      .on_click(() => {
        if (on) {
          run("layout.hide", { id: "panel" });
        } else {
          showPanel(id);
        }
      })
      .child(svg(icon).size(14))
      .child(label);
  }

  // Where the system draws none (Linux, Windows), the bar draws the window buttons.
  windowButtons(el, w, t) {
    const win = w.platform === "windows";
    const icon = (name) => svg(`mods/tabs-clock/${name}.svg`).size(16);
    return el
      .child(this.button("minimize", win ? icon("win-min") : "−", () => run("window.minimize"), t))
      .child(
        this.button(
          "maximize",
          win ? icon(w.maximized ? "win-restore" : "win-max") : "□",
          () => run("window.maximize"),
          t,
        ),
      )
      .child(
        this.button(
          "close-window",
          win ? icon("win-close") : svg("mods/pane/close.svg").size(13),
          () => run("window.close"),
          t,
        ),
      );
  }

  // Laid over the bar without listeners, so the tabs and window dragging under it still work, and
  // before them, so the tabs sit on its rainbow. Stretched left under the sidebar, so the clock
  // sits in the middle of the whole window; it starts its rainbow at the bar's own left edge.
  clock(root) {
    return div()
      .absolute()
      .top(0)
      .bottom(0)
      .right(0)
      .left(-sidebarWidth(root))
      .child(this.clockCat);
  }

  render() {
    const t = theme();
    const space = spaces().find((p) => p.active);
    const shown = space?.tabs ?? [];
    const active = shown.findIndex((tab) => tab.active);
    this.follow(shown, active);
    const root = layout();
    const w = window();
    const panel = find(root, "panel");
    const body = find(root, "panel-body");
    const enabled = shared("workspace.panels") ?? ["files", "changes"];
    this.syncPanel(space, panel, body, enabled);
    return h_flex()
      .relative()
      .size_full()
      .min_w(0)
      .px(10)
      .pl(lightsPadding(root, w))
      .gap(3)
      .items_center()
      .bg(t.chrome)
      .border_b(1)
      .border_color(t.border)
      .child(this.clock(root))
      .on_mouse_up("left", () => {
        this.pressed = null;
      })
      .on_scroll_wheel((e) => {
        if (Math.abs(e.delta.y) >= 1) {
          run(e.delta.y > 0 ? "tab.prev" : "tab.next");
        }
      })
      .child(
        h_flex()
          .id("tab-scroll")
          .flex_1()
          .min_w(0)
          .h_full()
          .items_center()
          .gap(3)
          .overflow_x_scrollbar()
          .track_scroll(this.scroll)
          .on_size_change((_size, cx) => {
            if (active >= 0) {
              this.scroll.scroll_to_item(active);
              cx.notify();
            }
          })
          .children(shown.map((c, i) => this.tabItem({ c, i, active, shown, space }, t))),
      )
      .child(
        this.button(
          "tab-new",
          "+",
          () => openOverlay({ mod: "new-menu", kind: "popup", width: 250, height: 512 }),
          t,
        ),
      )
      .when(enabled.includes("files"), (el) =>
        el.child(
          this.panelButton(
            { id: "files", label: "Files", icon: "mods/files/file.svg", panel, body },
            t,
          ),
        ),
      )
      .when(enabled.includes("changes"), (el) =>
        el.child(
          this.panelButton(
            { id: "changes", label: "Changes", icon: "mods/changes/changes.svg", panel, body },
            t,
          ),
        ),
      )
      .when(Boolean(w?.buttons), (el) => this.windowButtons(el, w, t));
  }
}
