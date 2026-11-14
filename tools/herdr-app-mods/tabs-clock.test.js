import { afterEach, beforeEach, expect, test, vi } from "vitest";

import { element } from "./mocks/gpui-kit.js";
import { host, reset } from "./mocks/herdr.js";
import Tabs from "../../.config/herdr-app/mods/tabs-clock/main.js";
import { ClockCat } from "../../.config/herdr-app/mods/tabs-clock/clock.js";

const T = new Proxy({}, { get: (_t, k) => k });

let store;
beforeEach(() => {
  store = new Map();
  vi.stubGlobal("localStorage", {
    getItem: (k) => store.get(k) ?? null,
    setItem: (k, v) => store.set(k, v),
  });
  vi.useFakeTimers({ toFake: ["Date"], now: new Date(2026, 10, 14, 9, 5, 30) });
  host.theme = T;
});

afterEach(() => {
  reset();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

function fakeCx() {
  const cx = {
    everies: [],
    timer: { every: (ms, fn) => cx.everies.push({ ms, fn }) },
    notify: vi.fn(),
    stop_propagation: vi.fn(),
    new: (Cls, props) => ({ Cls, props }),
  };
  return cx;
}

function mount() {
  const cx = fakeCx();
  const view = new Tabs();
  view.init({}, cx);
  return { view, cx };
}

const nodes = (el) => (el?.kids ? [el, ...el.kids.flatMap(nodes)] : []);
const byId = (root, id) =>
  nodes(root).find((n) => n.calls.some(([k, v]) => k === "id" && v === id));
const handler = (el, name, button) =>
  el.calls.find(([k, a]) => k === name && (button === undefined || a === button)).at(-1);
const has = (el, name) => el.calls.some(([k]) => k === name);
const runs = () => host.calls.filter(([n]) => n === "run").map(([, ...a]) => a);
const svgs = (el) =>
  nodes(el)
    .filter((n) => n.kind === "svg")
    .map((n) => n.path);

const tab = (id, extra = {}) => ({ id, name: id, active: false, status: null, ...extra });
const panelLayout = (hidden, active = "files") => ({
  id: "root",
  traffic_lights: [12, 16],
  children: [
    { id: "sidebar", size: { px: 268 } },
    { id: "panel", hidden, children: [{ id: "panel-body", active }] },
  ],
});

test("the nyan cat clock sits under the tabs across the whole window, letting clicks through", () => {
  const { view, cx } = mount();
  expect(view.clockCat).toStrictEqual({ Cls: ClockCat, props: {} });
  const root = view.render(cx);
  const clock = root.kids[0];
  expect(clock.kids).toStrictEqual([view.clockCat]);
  expect(has(clock, "absolute")).toBe(true);
  // Without a sidebar it spans just the bar; with one it reaches under it to the window's edge.
  expect(clock.calls.find(([k]) => k === "left")).toStrictEqual(["left", -0]);
  host.layout = panelLayout(true);
  expect(view.render(cx).kids[0].calls.find(([k]) => k === "left")).toStrictEqual(["left", -268]);
  // No listeners: clicks and drags reach the bar under it.
  expect(clock.calls.some(([k]) => k.startsWith("on_"))).toBe(false);
});

test("tabs show their program, status and close button; clicks select, move, rename and close", () => {
  host.panes = [
    { tab: "a", program: { name: "zsh", icon: "icons/zsh.svg", tint: true } },
    { tab: "b", program: { name: "zsh" } },
    { tab: "b", program: { name: "Claude Code", agent: true, icon: "icons/c.png", tint: false } },
  ];
  host.spaces = [
    {
      id: "s",
      active: true,
      tabs: [
        tab("tb1", { status: "working" }),
        tab("b", { active: true, zoomed: true, status: "blocked" }),
        tab("a"),
      ],
    },
  ];
  host.panes[0].tab = "tb1";
  const { view, cx } = mount();
  const root = view.render(cx);
  expect(view.scroll.items).toStrictEqual([1]);
  const first = byId(root, "tab-tb1");
  const active = byId(root, "tab-b");
  expect(nodes(first).some((n) => n.kids.includes("zsh"))).toBe(true);
  expect(nodes(active).some((n) => n.kids.includes("b Z"))).toBe(true);
  expect(svgs(first)).toContain("mods/workspace-chrome/spinner.svg");
  expect(nodes(active).some((n) => n.kind === "image" && n.path === "icons/c.png")).toBe(true);
  expect(byId(root, "close-b")).toBeTruthy();
  expect(byId(root, "close-a")).toBeUndefined();
  expect(nodes(byId(root, "tab-a")).some((n) => n.kids.includes("a"))).toBe(true);
  handler(first, "hover")(element("style"));

  // A size change before the active tab keeps it in view.
  handler(first, "on_size_change")(null, cx);
  expect(view.scroll.items).toStrictEqual([1, 1]);
  expect(has(byId(root, "tab-a"), "on_size_change")).toBe(false);

  // Press and release on one tab selects it; a double press renames.
  handler(first, "on_mouse_down", "left")({ click_count: 2 }, cx);
  handler(first, "on_mouse_up", "left")();
  // Dragged from the first onto the last: moved after it.
  handler(first, "on_mouse_down", "left")({ click_count: 1 }, cx);
  handler(byId(root, "tab-a"), "on_mouse_up", "left")();
  handler(active, "on_mouse_down", "right")(null, cx);
  handler(byId(root, "close-b"), "on_click")(null, cx);
  expect(runs()).toStrictEqual([
    ["tab.rename", { id: "tb1" }],
    ["tab.select", { id: "tb1" }],
    ["tab.move", { id: "tb1", index: 3 }],
    ["tab.close", { id: "b" }],
  ]);
  expect(host.calls.find(([n]) => n === "openOverlay")[1].props).toStrictEqual({
    tab: "tab",
    target: { id: "b", space: "s" },
  });
  // The bar's own release clears a press that ended elsewhere; the wheel steps through tabs.
  handler(first, "on_mouse_down", "left")({ click_count: 1 }, cx);
  handler(root, "on_mouse_up", "left")();
  expect(view.pressed).toBeNull();
  handler(root, "on_scroll_wheel")({ delta: { y: 2 } });
  handler(root, "on_scroll_wheel")({ delta: { y: -2 } });
  handler(root, "on_scroll_wheel")({ delta: { y: 0.5 } });
  expect(runs().slice(-2)).toStrictEqual([["tab.prev"], ["tab.next"]]);

  const scroller = byId(root, "tab-scroll");
  handler(scroller, "on_size_change")(null, cx);
  expect(view.scroll.items).toStrictEqual([1, 1, 1]);
  // A render with the same selection doesn't scroll again.
  view.render(cx);
  expect(view.scroll.items).toHaveLength(3);
});

test("no active space: an empty bar, nothing to keep in view", () => {
  const { view, cx } = mount();
  const root = view.render(cx);
  handler(byId(root, "tab-scroll"), "on_size_change")(null, cx);
  expect(view.scroll.items).toStrictEqual([]);
  handler(byId(root, "tab-new"), "on_click")();
  handler(byId(root, "tab-new"), "hover")(element("style"));
  expect(host.calls.at(-1)[1].mod).toBe("new-menu");
});

test("the bar leaves room for the macOS traffic lights unless the sidebar covers them", () => {
  const { view, cx } = mount();
  const left = (root) => root.calls.findLast(([k]) => k === "pl")[1];
  host.window = { platform: "macos", titlebar: "none", fullscreen: false };
  host.layout = panelLayout(true);
  expect(left(view.render(cx))).toBe(10);
  host.layout.children[0] = { id: "sidebar", hidden: true };
  expect(left(view.render(cx))).toBe(76);
  host.layout.children[0] = { id: "sidebar" };
  expect(left(view.render(cx))).toBe(76);
  host.window.fullscreen = true;
  expect(left(view.render(cx))).toBe(10);
});

test("panel buttons toggle their panel, which each space remembers", () => {
  host.spaces = [{ id: "s1", active: true, tabs: [] }];
  host.layout = panelLayout(true);
  const { view, cx } = mount();
  let root = view.render(cx);
  // The first space never chose: nothing is stored or restored.
  expect(store.get("panels")).toBe('{"s1":null}');
  handler(byId(root, "panel-files"), "on_click")();
  expect(runs()).toStrictEqual([
    ["layout.select", { stack: "panel-body", child: "files" }],
    ["layout.show", { id: "panel" }],
  ]);
  host.layout = panelLayout(false, "changes");
  root = view.render(cx);
  expect(JSON.parse(store.get("panels"))).toStrictEqual({ s1: "changes" });
  handler(byId(root, "panel-changes"), "on_click")();
  expect(runs().at(-1)).toStrictEqual(["layout.hide", { id: "panel" }]);
  handler(byId(root, "panel-files"), "hover")(element("style"));

  // Switching to a space that had it closed closes it, and waits for that to land.
  host.calls.length = 0;
  view.panels.s2 = null;
  host.spaces = [{ id: "s2", active: true, tabs: [] }];
  view.render(cx);
  expect(runs()).toStrictEqual([["layout.hide", { id: "panel" }]]);
  view.render(cx);
  expect(view.panels.s2).toBeNull();
  // Once it lands (or a second has passed), the state is the space's choice again.
  vi.setSystemTime(Date.now() + 1500);
  view.render(cx);
  expect(view.panels.s2).toBe("changes");

  // Back to s1, whose panel was "changes" and still is: no restore needed.
  host.calls.length = 0;
  host.spaces = [{ id: "s1", active: true, tabs: [] }];
  view.render(cx);
  expect(runs()).toStrictEqual([]);

  // A space that had "files" open gets it back; a panel no longer enabled counts as closed.
  view.panels.s3 = "files";
  host.spaces = [{ id: "s3", active: true, tabs: [] }];
  view.render(cx);
  expect(runs()).toStrictEqual([
    ["layout.select", { stack: "panel-body", child: "files" }],
    ["layout.show", { id: "panel" }],
  ]);
  host.layout = panelLayout(false, "files");
  view.render(cx);
  expect(view.pending).toBeNull();
  host.shared["workspace.panels"] = [];
  host.calls.length = 0;
  view.panels.s4 = "files";
  host.spaces = [{ id: "s4", active: true, tabs: [] }];
  root = view.render(cx);
  expect(runs()).toStrictEqual([["layout.hide", { id: "panel" }]]);
  expect(byId(root, "panel-files")).toBeUndefined();
});

test("a layout without the panel stores nothing", () => {
  host.spaces = [{ id: "s1", active: true, tabs: [] }];
  const { view, cx } = mount();
  view.render(cx);
  expect(store.has("panels")).toBe(false);
});

test("where the system draws no window buttons, the bar does: vector icons on Windows", () => {
  const { view, cx } = mount();
  host.window = { platform: "windows", buttons: true, maximized: false };
  let root = view.render(cx);
  expect(svgs(root).filter((p) => p.startsWith("mods/tabs-clock/"))).toStrictEqual([
    "mods/tabs-clock/win-min.svg",
    "mods/tabs-clock/win-max.svg",
    "mods/tabs-clock/win-close.svg",
  ]);
  host.window.maximized = true;
  expect(svgs(view.render(cx))).toContain("mods/tabs-clock/win-restore.svg");
  host.window = { platform: "linux", buttons: true };
  root = view.render(cx);
  expect(byId(root, "minimize").kids).toStrictEqual(["−"]);
  expect(byId(root, "maximize").kids).toStrictEqual(["□"]);
  for (const id of ["minimize", "maximize", "close-window"]) {
    handler(byId(root, id), "on_click")();
  }
  expect(runs()).toStrictEqual([["window.minimize"], ["window.maximize"], ["window.close"]]);
});
