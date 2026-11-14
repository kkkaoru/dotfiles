import { afterEach, beforeEach, expect, test, vi } from "vitest";

import { element } from "./mocks/gpui-kit.js";
import { host, reset } from "./mocks/herdr.js";
import Agents from "../../.config/herdr-app/mods/agents-emoji/main.js";
import { NYAN_MS, NyanCat } from "../../.config/herdr-app/mods/agents-emoji/nyan.js";

const NOW = 100 * 86400000;
const DAY = 86400000;
const T = {
  popover: "#pop",
  dim: "#dim",
  foreground: "#fg",
  hover: "#hover",
  muted: "#muted",
  raised: "#raised",
  input: "#input",
  working: "#working",
  blocked: "#b",
  done: "#done",
  idle: "#idle",
  selected: "#sel",
  chrome: "#chrome",
  border: "#border",
};
const CLAUDE = { key: "claude", name: "Claude", agent: true };
const CODEX = { key: "codex", name: "Codex", agent: true };
const wide = () => ({
  id: "root",
  children: [{ id: "main" }, { id: "sidebar", size: { px: 300 } }],
});
const thin = () => ({ id: "root", children: [{ id: "sidebar", size: { px: 100 } }] });

let store;
beforeEach(() => {
  store = new Map();
  vi.stubGlobal("localStorage", {
    getItem: vi.fn((k) => store.get(k) ?? null),
    setItem: vi.fn((k, v) => store.set(k, String(v))),
    removeItem: vi.fn((k) => store.delete(k)),
  });
  vi.spyOn(Date, "now").mockReturnValue(NOW);
  host.theme = T;
  host.layout = wide();
});

afterEach(() => {
  reset();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const agent = (address, o = {}) => ({
  address,
  pane: `pane-${address}`,
  space: "s1",
  space_name: "alpha",
  tab: `tab-${address}`,
  tab_index: 1,
  tab_name: `tn-${address}`,
  status: "idle",
  program: CLAUDE,
  title: "",
  seq: 0,
  pinned: false,
  focused: false,
  ...o,
});

function newView() {
  const every = [];
  const cx = {
    timer: { every: (ms, fn) => every.push({ ms, fn }) },
    notify: vi.fn(),
    new: vi.fn((Cls, p) => ({ Cls, p, set_props: vi.fn(), release: vi.fn() })),
  };
  const view = new Agents();
  view.init({}, cx);
  return { view, every, cx };
}

const walk = (node) => (node?.calls ? [node, ...node.kids.flatMap(walk)] : []);
const idOf = (node) => node.calls.find((c) => c[0] === "id")?.[1];
const ids = (tree) => walk(tree).map(idOf).filter(Boolean);
const byId = (tree, id) => walk(tree).find((n) => idOf(n) === id);
const arg = (node, name) => node.calls.find((c) => c[0] === name)?.[1];
const texts = (node) => {
  if (node?.calls) {
    return node.kids.flatMap(texts);
  }
  return typeof node === "string" ? [node] : [];
};
const paths = (tree) =>
  walk(tree)
    .map((n) => n.path)
    .filter(Boolean);
const ecx = () => ({ stop_propagation: vi.fn(), notify: vi.fn() });
const click = (tree, id, cx = ecx()) => {
  arg(byId(tree, id), "on_click")(null, cx);
  return cx;
};
// Calls every recorded hover style on a probe and returns what each asked for.
const hovers = (tree) =>
  walk(tree)
    .filter((n) => arg(n, "hover"))
    .map((n) => arg(n, "hover")(element("probe")).calls);

test("the sort menu marks the current order and picks another", () => {
  host.props = { sortMenu: true, overlay: "o1" };
  const { view, every } = newView();
  expect(every).toStrictEqual([]);
  expect(view.folded).toBe(undefined);
  const menu = view.render();
  expect(texts(menu)).toStrictEqual(["Sort agents", "Priority", "Grouped by space"]);
  expect(paths(menu)).toStrictEqual([
    "mods/workspace-chrome/clock.svg",
    "mods/workspace-chrome/check.svg",
    "mods/workspace-chrome/layers.svg",
  ]);
  expect(hovers(menu)).toStrictEqual([[["bg", "#hover"]], [["bg", "#hover"]]]);
  click(menu, "sort-grouped");
  expect(host.calls).toStrictEqual([
    ["setShared", "agents.sort", "grouped"],
    ["closeOverlay", "o1"],
  ]);
  expect(paths(view.render())).toStrictEqual([
    "mods/workspace-chrome/clock.svg",
    "mods/workspace-chrome/layers.svg",
    "mods/workspace-chrome/check.svg",
  ]);
});

test("with no agents the list says why and the sort button opens its menu", () => {
  const { view } = newView();
  expect(localStorage.removeItem).toHaveBeenCalledWith("pins");
  const tree = view.render();
  expect(texts(tree)).toStrictEqual([
    "Agents",
    "No agent status yet. Agents show up here when they report their status. Older versions and agents inside tmux may not.",
  ]);
  expect(arg(byId(tree, "ag-sort"), "tooltip")).toBe("Sort agents: Priority");
  expect(hovers(byId(tree, "ag-sort"))).toStrictEqual([[["text_color", "#fg"]]]);
  host.calls.length = 0;
  click(tree, "ag-sort");
  expect(host.calls).toStrictEqual([
    [
      "openOverlay",
      {
        mod: "agents-emoji",
        kind: "popup",
        width: 200,
        height: 140,
        props: { sortMenu: true },
      },
    ],
  ]);
});

const priorityHost = () => {
  host.agents = [
    agent("p1", { seq: 1 }),
    agent("b1", { status: "blocked", seq: 2 }),
    agent("d1", { status: "done", seq: 3 }),
    agent("w1", { status: "working", seq: 4, title: "✳ Busy" }),
    agent("i1", { seq: 5 }),
    agent("i2", { seq: 6 }),
    agent("i3", { seq: 7 }),
    agent("i4", { seq: 8 }),
    agent("x1", { status: "weird", seq: 9 }),
    agent("k2", { status: "working", seq: 10 }),
  ];
  host.terminals = [
    {
      address: "k1",
      parent: "p1",
      state: "live",
      status: "error",
      name: "k-one",
      program: { icon: "i.svg", tint: true },
    },
    {
      address: "k2",
      parent: "p1",
      state: "live",
      status: "idle",
      template: "tmpl",
      program: { icon: "p.png", tint: false },
    },
    { address: "k3", parent: "k1", state: "live", status: "working", program: { name: "Kid" } },
    { address: "k4", parent: "k1", state: "live" },
    { address: "k5", parent: "k2", state: "live", status: "done" },
    { address: "k6", parent: "p1", state: "exited" },
  ];
  host.focused = { agent: "k1" };
  store.set(
    "changed",
    JSON.stringify({ i1: { s: "idle", at: NOW - 1000 }, i2: { s: "idle", at: NOW - 5000 } }),
  );
};

test("priority puts the most urgent first, a parent ranked by its sub-agents, ties by change then seq", () => {
  priorityHost();
  const { view } = newView();
  const tree = view.render();
  expect(host.calls[0]).toStrictEqual([
    "setAgentOrder",
    ["b1", "p1", "d1", "w1", "i4", "i3", "i1", "i2", "x1"],
  ]);
  expect(ids(tree)).toStrictEqual([
    "ag-sort",
    "ag-list",
    "sec-4",
    "ag-b1",
    "ag-p1",
    "fold-p1",
    "sub-k1",
    "sub-k3",
    "sub-k4",
    "sub-k2",
    "sub-k5",
    "sec-3",
    "ag-d1",
    "ag-w1",
    "ag-i4",
    "ag-i3",
    "ag-i1",
    "ag-i2",
    "ag-x1",
  ]);
  expect(texts(byId(tree, "sec-4"))).toStrictEqual(["Needs you", "2"]);
  expect(arg(byId(tree, "sec-4").kids[0], "text_color")).toBe("#b");
  expect(texts(byId(tree, "sec-3"))).toStrictEqual(["Done", "1"]);
  // Without a space header a row names its space.
  expect(arg(byId(tree, "ag-w1"), "tooltip")).toBe("alpha · ✳ Busy");
  expect(arg(byId(tree, "ag-b1"), "tooltip")).toBe("alpha · Claude");
  expect(byId(tree, "ag-w1").kids.length).toBe(1);
  expect(texts(byId(tree, "ag-b1").kids.at(-1))).toStrictEqual(["!"]);
});

test("sub-agents hang under their parent with tree lines, names and states", () => {
  priorityHost();
  const { view } = newView();
  const tree = view.render();
  const lines = (id) =>
    byId(tree, id)
      .kids.filter((k) => k.calls?.[0]?.[0] === "self_start")
      .map((k) => k.calls.filter((c) => c[0].startsWith("border")).map((c) => c[0]));
  expect(lines("sub-k1")).toStrictEqual([["border_l", "border_color", "border_b"]]);
  expect(lines("sub-k4")).toStrictEqual([
    ["border_l", "border_color"],
    ["border_l", "border_color", "border_b"],
  ]);
  expect(lines("sub-k5")).toStrictEqual([[], ["border_l", "border_color", "border_b"]]);
  expect(
    ["sub-k1", "sub-k2", "sub-k3", "sub-k4"].map((id) => arg(byId(tree, id), "tooltip")),
  ).toStrictEqual(["k-one", "tmpl", "Kid", "k4"]);
  expect(byId(tree, "sub-k1").calls).toContainEqual(["bg", "#raised"]);
  expect(byId(tree, "sub-k2").calls).not.toContainEqual(["bg", "#raised"]);
  expect(paths(byId(tree, "sub-k1"))[0]).toBe("i.svg");
  expect(
    arg(
      walk(byId(tree, "sub-k1")).find((n) => n.path === "i.svg"),
      "text_color",
    ),
  ).toBe("#fg");
  expect(walk(byId(tree, "sub-k2")).find((n) => n.path === "p.png").kind).toBe("image");
  // The name rides the cat's rainbow; the row's own text is its state.
  expect(texts(byId(tree, "sub-k1"))).toStrictEqual(["!"]);
  expect(view.wanted.get("k1").name).toBe("k-one");
  // A kept sub-agent's state comes from agents(): k2 is working there, so no state mark.
  expect(byId(tree, "sub-k2").kids.at(-1).kind).toBe("h_flex");
  expect(arg(byId(tree, "sub-k5").kids.at(-1), "border")).toBe(1.5);
  expect(arg(byId(tree, "sub-k4").kids.at(-1), "bg")).toBe("#dim00");
  click(tree, "sub-k2");
  click(tree, "sub-k1");
  expect(host.calls.slice(1)).toStrictEqual([
    ["run", "pane.focus", { terminal: "k2" }],
    ["run", "terminal.peek", { terminal: "k1" }],
  ]);
  expect(hovers(byId(tree, "sub-k1"))).toStrictEqual([[["bg", "#hover"]]]);
});

test("a parent's fold hides its sub-agents and shows how many, colored by the most urgent", () => {
  priorityHost();
  const { view } = newView();
  const tree = view.render();
  expect(paths(byId(tree, "fold-p1"))).toStrictEqual(["mods/workspace-chrome/chevron-down.svg"]);
  expect(arg(byId(tree, "fold-p1"), "text_color")).toBe("#dim");
  expect(hovers(byId(tree, "fold-p1"))).toStrictEqual([
    [
      ["bg", "#hover"],
      ["text_color", "#fg"],
    ],
  ]);
  const cx = click(tree, "fold-p1");
  expect(cx.stop_propagation).toHaveBeenCalledTimes(1);
  expect(cx.notify).toHaveBeenCalledTimes(1);
  const folded = view.render();
  expect(ids(folded).filter((id) => id.startsWith("sub-"))).toStrictEqual([]);
  expect(texts(byId(folded, "fold-p1"))).toStrictEqual(["5"]);
  expect(arg(byId(folded, "fold-p1"), "text_color")).toBe("#b");
  expect(paths(byId(folded, "fold-p1"))).toStrictEqual(["mods/workspace-chrome/chevron-right.svg"]);
  click(folded, "fold-p1");
  expect(ids(view.render())).toContain("sub-k5");
});

test("a fold whose sub-agents report nothing known keeps the dim color", () => {
  host.agents = [agent("p1")];
  host.terminals = [{ address: "k1", parent: "p1", state: "live", status: "weird" }];
  const { view } = newView();
  click(view.render(), "fold-p1");
  expect(arg(byId(view.render(), "fold-p1"), "text_color")).toBe("#dim");
});

test("a priority section folds behind its header and opens again", () => {
  priorityHost();
  const { view } = newView();
  const tree = view.render();
  expect(hovers(byId(tree, "sec-4"))[0]).toStrictEqual([["text_color", "#fg"]]);
  const cx = click(tree, "sec-4");
  expect(cx.notify).toHaveBeenCalledTimes(1);
  const folded = view.render();
  expect(ids(folded).slice(2, 5)).toStrictEqual(["sec-4", "sec-3", "ag-d1"]);
  expect(texts(byId(folded, "sec-4"))).toStrictEqual(["Needs you", "2"]);
  expect(paths(byId(folded, "sec-4"))).toStrictEqual(["mods/workspace-chrome/chevron-right.svg"]);
  click(folded, "sec-4");
  expect(ids(view.render()).slice(2, 4)).toStrictEqual(["sec-4", "ag-b1"]);
});

const space = (id, name, tabs) => ({ id, name, tabs });
const tab = (id, name, named = false) => ({ id, name, named });
const groupedHost = () => {
  host.shared["agents.sort"] = "grouped";
  host.spaces = [
    space("s1", "alpha", [
      tab("tA", "build", true),
      tab("tB", "2"),
      tab("tC", "3"),
      tab("tD", "4"),
      tab("tH", "pair", true),
    ]),
    space("s2", "beta", [tab("tE2", "Claude", true), tab("tE", "5")]),
    space("s3", "gamma", [tab("tF", "6")]),
  ];
  host.panes = [
    { terminal: null, tab: "tB", title: "mod" },
    { terminal: "x1", tab: "tB", title: "shell", program: { agent: false }, focused: false },
    { terminal: "x2", tab: "tB", title: "✳ Focused", program: { agent: false }, focused: true },
    { terminal: "x2b", tab: "tB", title: "zsh", program: { agent: false }, focused: false },
    { terminal: "x3", tab: "tB", title: "✳ Fixing bug", program: CLAUDE },
    { terminal: "x4", tab: "tB", title: "other", program: CLAUDE },
    { terminal: "c", tab: "tC", title: "✳ ~/src", program: CLAUDE },
    { terminal: "d", tab: "tD", title: "me@host", program: CLAUDE },
    { terminal: "e2", tab: "tE2", title: "✳ Deploy", program: CLAUDE },
    { terminal: "e", tab: "tE", title: "Codex", program: CODEX },
    { terminal: "f", tab: "tF", title: "codex", program: { name: "Cx", key: "codex" } },
  ];
  host.agents = [
    agent("A", { tab: "tA", focused: true }),
    agent("B", { tab: "tB", status: "blocked" }),
    agent("C", { tab: "tC" }),
    agent("D", { tab: "tD" }),
    agent("H1", { tab: "tH", title: "✳ Alpha task" }),
    agent("H2", { tab: "tH", title: "C:\\x" }),
    agent("E2", { space: "s2", space_name: "beta", tab: "tE2" }),
    agent("E", { space: "s2", space_name: "beta", tab: "tE", program: CODEX }),
    agent("F", { space: "s3", space_name: "gamma", tab: "tF", program: { name: "", key: "x" } }),
    agent("G1", { space: "s4", space_name: "", title: "solo" }),
    agent("G2", { space: "s4", space_name: "", program: { name: "Pi" } }),
    agent("G3", { space: "s4", space_name: "", program: undefined }),
    agent("vps/t7", { space: "s5", space_name: "delta", status: "weird", title: "remote" }),
  ];
};

test("grouped rows read their tab or the agent's own title, a tab number only to tell two apart", () => {
  groupedHost();
  const { view } = newView();
  const tree = view.render();
  expect(ids(tree)).toStrictEqual([
    "ag-sort",
    "ag-list",
    "sp-s1",
    "ag-A",
    "ag-B",
    "ag-C",
    "ag-D",
    "ag-H1",
    "ag-H2",
    "sp-s2",
    "ag-E2",
    "ag-E",
    "sp-s3",
    "ag-F",
    "sp-s4",
    "ag-G1",
    "ag-G2",
    "ag-G3",
    "sp-s5",
    "ag-vps/t7",
  ]);
  expect(
    ["A", "B", "C", "D", "H1", "H2", "E2", "E", "F", "G1", "G2", "G3", "vps/t7"].map((a) =>
      arg(byId(tree, `ag-${a}`), "tooltip"),
    ),
  ).toStrictEqual([
    "build",
    "Fixing bug",
    "3",
    "4",
    "Alpha task",
    "pair",
    "Deploy",
    "Codex",
    "6",
    "solo",
    "Pi",
    "",
    "remote",
  ]);
  expect(byId(tree, "ag-A").calls).toContainEqual(["bg", "#sel"]);
  expect(texts(byId(tree, "ag-vps/t7"))).toStrictEqual(["vps"]);
  expect(view.wanted.get("vps/t7")).toMatchObject({ name: "remote", since: expect.any(Number) });
  // A space header without a name gets a plain label; a group's count shows only when folded.
  expect(byId(tree, "sp-s4").kids.map((k) => k.kind)).toStrictEqual(["div", "div"]);
  expect(arg(byId(tree, "sp-s4").kids[0], "text_color")).toBe("#dim");
  expect(byId(tree, "sp-s1").kids.map((k) => k.kind)).toStrictEqual(["h_flex", "div"]);
  expect(hovers(byId(tree, "ag-A"))).toStrictEqual([[["bg", "#hover"]]]);
});

test("an automatic tab with no pane to read is its number, or a program-less agent's", () => {
  host.shared["agents.sort"] = "grouped";
  host.spaces = [space("s1", "alpha", [tab("tG", "7")])];
  host.agents = [agent("G", { tab: "tG", program: undefined })];
  expect(arg(byId(newView().view.render(), "ag-G"), "tooltip")).toBe("7");
});

test("a folded space header says how many and its most urgent state unless that is idle", () => {
  groupedHost();
  const { view } = newView();
  const tree = view.render();
  click(tree, "sp-s1");
  click(tree, "sp-s4");
  click(tree, "sp-s5");
  const folded = view.render();
  expect(ids(folded)).toStrictEqual([
    "ag-sort",
    "ag-list",
    "sp-s1",
    "sp-s2",
    "ag-E2",
    "ag-E",
    "sp-s3",
    "ag-F",
    "sp-s4",
    "sp-s5",
  ]);
  expect(byId(folded, "sp-s1").kids.map((k) => k.kind)).toStrictEqual([
    "h_flex",
    "div",
    "div",
    "div",
    "svg",
  ]);
  expect(texts(byId(folded, "sp-s1").kids[1])).toStrictEqual(["6"]);
  expect(texts(byId(folded, "sp-s1").kids[3])).toStrictEqual(["!"]);
  expect(texts(byId(folded, "sp-s4"))).toStrictEqual(["", "3"]);
  expect(byId(folded, "sp-s4").kids.map((k) => k.kind)).toStrictEqual(["div", "div", "div", "svg"]);
  expect(byId(folded, "sp-s5").kids.map((k) => k.kind)).toStrictEqual([
    "h_flex",
    "div",
    "div",
    "svg",
  ]);
  click(folded, "sp-s1");
  expect(ids(view.render())).toContain("ag-A");
});

test("a hovered row shows its pin, which pins the agent and drops the hover", () => {
  host.agents = [agent("a1"), agent("a2")];
  const { view } = newView();
  const row = byId(view.render(), "ag-a1");
  const cx = ecx();
  arg(row, "on_hover")(true, cx);
  expect(view.hovered).toBe("a1");
  arg(byId(view.render(), "ag-a2"), "on_hover")(false, cx);
  expect(view.hovered).toBe("a1");
  expect(cx.notify).toHaveBeenCalledTimes(2);
  const tree = view.render();
  expect(arg(byId(tree, "pin-a1"), "tooltip")).toBe("Pin to the top");
  expect(paths(byId(tree, "pin-a1"))).toStrictEqual(["mods/workspace-chrome/pin.svg"]);
  expect(hovers(byId(tree, "pin-a1"))).toStrictEqual([
    [
      ["bg", "#hover"],
      ["text_color", "#fg"],
    ],
  ]);
  host.calls.length = 0;
  const pinCx = click(tree, "pin-a1");
  expect(pinCx.stop_propagation).toHaveBeenCalledTimes(1);
  expect(host.calls).toStrictEqual([["run", "terminal.pin", { terminal: "a1", pinned: true }]]);
  expect(view.hovered).toBe(null);
  arg(byId(view.render(), "ag-a1"), "on_hover")(true, cx);
  arg(byId(view.render(), "ag-a1"), "on_hover")(false, cx);
  expect(view.hovered).toBe(null);
  const after = view.render();
  host.calls.length = 0;
  click(after, "ag-a1");
  expect(host.calls).toStrictEqual([["run", "pane.focus", { pane: "pane-a1" }]]);
});

test("pinned agents lead under their own header and name their space", () => {
  host.shared["agents.sort"] = "grouped";
  host.agents = [agent("a1", { status: "blocked" }), agent("a2", { pinned: true, title: "job" })];
  host.terminals = [{ address: "a2", parent: "a1", state: "live" }];
  const { view } = newView();
  const tree = view.render();
  expect(ids(tree)).toStrictEqual([
    "ag-sort",
    "ag-list",
    "pinned",
    "ag-a2",
    "sp-s1",
    "ag-a1",
    "fold-a1",
    "sub-a2",
  ]);
  expect(texts(byId(tree, "pinned"))).toStrictEqual(["Pinned"]);
  expect(arg(byId(tree, "ag-a2"), "tooltip")).toBe("alpha · job");
  arg(byId(tree, "ag-a2"), "on_hover")(true, ecx());
  const hovered = view.render();
  expect(arg(byId(hovered, "pin-a2"), "tooltip")).toBe("Unpin");
  expect(paths(byId(hovered, "pin-a2"))).toStrictEqual(["mods/workspace-chrome/pin-off.svg"]);
  host.calls.length = 0;
  click(hovered, "pin-a2");
  expect(host.calls).toStrictEqual([["run", "terminal.pin", { terminal: "a2", pinned: false }]]);
});

test("the rail lists numbered states with no headers or sub-agents, and widens again", () => {
  host.layout = thin();
  host.agents = [agent("a1", { status: "blocked" }), agent("k1", { status: "working" })];
  host.terminals = [{ address: "k1", parent: "a1", state: "live" }];
  host.shared["sidebar.width"] = 333;
  const { view } = newView();
  const tree = view.render();
  expect(ids(tree)).toStrictEqual(["ag-list", "ag-a1", "ag-k1", "collapse"]);
  expect(texts(tree)).toStrictEqual(["1", "!", "2", "»"]);
  expect(arg(byId(tree, "ag-list"), "px")).toBe(0);
  expect(hovers(tree)).toStrictEqual([
    [["bg", "#hover"]],
    [["bg", "#hover"]],
    [["text_color", "#fg"]],
  ]);
  click(tree, "ag-a1");
  click(tree, "collapse");
  expect(host.calls.slice(1)).toStrictEqual([
    ["run", "pane.focus", { pane: "pane-a1" }],
    ["setShared", "sidebar.collapsed", false],
    ["run", "layout.resize", { id: "sidebar", px: 333 }],
  ]);
});

test("the rail falls back to the default width with none saved", () => {
  host.layout = thin();
  const tree = newView().view.render();
  expect(texts(tree)).toStrictEqual(["»"]);
  click(tree, "collapse");
  expect(host.calls.at(-1)).toStrictEqual(["run", "layout.resize", { id: "sidebar", px: 268 }]);
});

test("collapsing a wide sidebar saves its width and leaves room for the traffic lights on macOS", () => {
  host.layout = thin();
  const collapse = arg(byId(newView().view.render(), "collapse"), "on_click");
  host.layout = wide();
  host.calls.length = 0;
  collapse();
  host.window = { platform: "linux" };
  collapse();
  host.window = null;
  collapse();
  expect(host.calls).toStrictEqual([
    ["setShared", "sidebar.width", 300],
    ["setShared", "sidebar.collapsed", true],
    ["run", "layout.resize", { id: "sidebar", px: 76 }],
    ["setShared", "sidebar.width", 300],
    ["setShared", "sidebar.collapsed", true],
    ["run", "layout.resize", { id: "sidebar", px: 44 }],
    ["setShared", "sidebar.width", 300],
    ["setShared", "sidebar.collapsed", true],
    ["run", "layout.resize", { id: "sidebar", px: 44 }],
  ]);
});

test("a sidebar without a size, or no layout at all, counts as wide", () => {
  host.agents = [agent("a1", { space_name: "", title: "job" })];
  host.layout = { id: "root", children: [{ id: "sidebar" }] };
  const { view } = newView();
  // Priority without a space name: the row names itself by its label.
  expect(arg(byId(view.render(), "ag-a1"), "tooltip")).toBe("job · job");
  expect(view.width).toBe(480);
  expect(view.wanted.get("a1").width).toBe(480);
  host.layout = null;
  expect(ids(view.render())).toStrictEqual(["ag-sort", "ag-list", "ag-a1"]);
});

test("the cat task makes, updates and drops each row's cat view, redrawing only when one comes or goes", () => {
  host.agents = [agent("a1", { status: "working" }), agent("a2")];
  const { view, every, cx } = newView();
  expect(every.map((e) => e.ms)).toStrictEqual([NYAN_MS]);
  const tick = ecx();
  every[0].fn({ ...cx, notify: tick.notify });
  expect(cx.new).not.toHaveBeenCalled();
  view.render();
  every[0].fn({ ...cx, notify: tick.notify });
  expect(cx.new.mock.calls.map(([Cls, p]) => [Cls, p.h, p.width, p.working])).toStrictEqual([
    [NyanCat, 22, 300, true],
    [NyanCat, 22, 300, false],
  ]);
  expect(tick.notify).toHaveBeenCalledTimes(1);
  const cat = view.cats.get("a1").entity;
  const gone = view.cats.get("a2").entity;
  expect(byId(view.render(), "ag-a1").kids[0].kids.at(-1)).toBe(cat);
  every[0].fn({ ...cx, notify: tick.notify });
  every[0].fn({ ...cx, notify: tick.notify });
  expect(tick.notify).toHaveBeenCalledTimes(1);
  expect(cat.set_props).not.toHaveBeenCalled();
  host.agents = [agent("a1")];
  view.render();
  every[0].fn({ ...cx, notify: tick.notify });
  expect(cat.set_props.mock.calls[0][0].working).toBe(false);
  expect(view.cats.get("a1").key).toBe(JSON.stringify(cat.set_props.mock.calls[0][0]));
  expect(view.cats.has("a2")).toBe(false);
  expect(gone.release).toHaveBeenCalledTimes(1);
  expect(tick.notify).toHaveBeenCalledTimes(2);
});

test("a cat asked for before any render is a still default cat", () => {
  const { view } = newView();
  const still = view.nyan("x", "working", 18);
  expect(still.kind).toBe("h_flex");
  expect(view.wanted.get("x")).toStrictEqual({
    h: 18,
    fur: 0,
    taste: 0,
    r: 0,
    width: undefined,
    working: true,
    name: undefined,
    since: undefined,
  });
});

test("state changes are stamped and kept, done counting as idle, and long-gone agents dropped", () => {
  store.set(
    "changed",
    JSON.stringify({
      old: { s: "idle", at: NOW - 31 * DAY },
      recent: { s: "idle", at: NOW - DAY },
      a2: { s: "idle", at: 5 },
    }),
  );
  host.agents = [agent("a1", { status: "working" }), agent("a2", { status: "done" })];
  const { view } = newView();
  view.render();
  expect(JSON.parse(store.get("changed"))).toStrictEqual({
    recent: { s: "idle", at: NOW - DAY },
    a2: { s: "idle", at: 5 },
    a1: { s: "working", at: NOW },
  });
  view.render();
  expect(localStorage.setItem).toHaveBeenCalledTimes(1);
  expect(view.since({ address: "a2" })).toBe(5);
  expect(view.since({ address: "none" })).toBe(0);
});

test("offline pins wait for their terminal, and each stays pending until core shows the label", () => {
  store.set("pins", JSON.stringify(["t1", "t2", "t3"]));
  host.terminals = [
    { address: "t1", state: "live" },
    { address: "t2", state: "live", labels: { "herdr.pinned": "1" } },
    { address: "t3", state: "exited" },
  ];
  const { view } = newView();
  expect(localStorage.removeItem).not.toHaveBeenCalled();
  view.render();
  view.render();
  expect(host.calls.filter((c) => c[1] === "terminal.pin")).toStrictEqual([
    ["run", "terminal.pin", { terminal: "t1", pinned: true }],
  ]);
  expect(JSON.parse(store.get("pins"))).toStrictEqual(["t1", "t3"]);
  host.terminals = [
    { address: "t1", state: "live", labels: { "herdr.pinned": "1" } },
    { address: "t3", state: "live", labels: { "herdr.pinned": "1" } },
  ];
  view.render();
  expect(store.has("pins")).toBe(false);
  expect(view.pendingPins).toStrictEqual([]);
});

test("a state mark: a spinner, a ! or a badge for blocked, a ring for idle and a dot otherwise", () => {
  const { view } = newView();
  const s = (status, compact, t = T) => view.status(t, { status }, compact);
  expect(s("working").path).toBe("mods/workspace-chrome/spinner.svg");
  expect(arg(s("working"), "spin")).toBe(900);
  expect(texts(s("blocked", true))).toStrictEqual(["!"]);
  expect(texts(s("blocked"))).toStrictEqual(["Needs you"]);
  expect(arg(s("blocked"), "bg")).toBe("#b26");
  expect(arg(s("idle"), "border_color")).toBe("#idle");
  expect(arg(s("idle", false, { dim: "#d" }), "border_color")).toBe("#d");
  expect(arg(s("done"), "bg")).toBe("#done");
  expect(arg(s("other"), "bg")).toBe("#dim00");
  expect(view.rank({ status: "other" })).toBe(0);
});

test("a cat keeps its fur, poptart and rainbow however long its agent stays in a state", () => {
  const { view } = newView();
  host.agents = [agent("a1"), agent("a2", { status: "working" })];
  view.render();
  const first = [...view.variants.values()];
  Date.now.mockReturnValue(NOW + 2 * 86_400_000);
  view.render();
  expect([...view.variants.values()]).toStrictEqual(first);
});

test("a cat keeps its fur, poptart and rainbow when its row moves", () => {
  const { view } = newView();
  host.agents = [agent("a1"), agent("a2"), agent("a3")];
  view.render();
  const first = new Map(view.variants);
  host.agents = [agent("a3"), agent("a1"), agent("a2")];
  view.render();
  expect(["a1", "a2", "a3"].map((a) => view.variants.get(a))).toStrictEqual(
    ["a1", "a2", "a3"].map((a) => first.get(a)),
  );
});
