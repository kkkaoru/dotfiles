import { afterEach, beforeEach, expect, test, vi } from "vitest";

import { element } from "./mocks/gpui-kit.js";
import { host, reset } from "./mocks/herdr.js";
import Spaces, {
  glyph,
  parseGpuPercent,
  parseMemoryPercent,
  tone,
} from "../../.config/herdr-app/mods/spaces-emoji/main.js";

const T = Object.fromEntries(
  [
    "chrome",
    "dim",
    "hover",
    "ansi1",
    "ansi2",
    "blocked",
    "muted",
    "foreground",
    "raised",
    "selected",
    "accent",
    "unknown",
  ].map((k) => [k, k]),
);
const OK = { code: 0, stdout: "", stderr: "" };
const REBUILT = 'a different core answers there ("new")';

const space = (id, extra = {}) => ({
  id,
  index: 0,
  name: id,
  named: true,
  folder: `/src/${id}`,
  machine: null,
  branch: null,
  active: false,
  status: null,
  tabs: [{ id: `tab-${id}` }],
  ...extra,
});
const pane = (id, tab, extra = {}) => ({
  pane: id,
  terminal: `term-${id}`,
  tab,
  cwd: null,
  status: null,
  program: { shell: true, agent: false },
  ...extra,
});

// Answers git per folder: { top, common, branch, counts } for a repo; { error } for git's no;
// { throws } for a machine away; { hang } for an answer that never comes.
function gitHost(specs) {
  host.exec = async (_cmd, args, { cwd }) => {
    const s = specs[cwd] ?? { error: "fatal: not a git repository (or any parent)" };
    if (s.throws) {
      throw new Error("away");
    }
    if (s.hang) {
      return new Promise(() => {});
    }
    if (s.error) {
      return { code: 128, stdout: "", stderr: s.error };
    }
    const fail = (stderr) => ({ code: 1, stdout: "", stderr });
    const ok = (stdout) => ({ code: 0, stdout, stderr: "" });
    const answers = {
      "--show-toplevel": ok(`${s.top}\n`),
      "--git-common-dir": s.commonError ? fail(s.commonError) : ok(`${s.common}\n`),
      HEAD: s.branch ? ok(`refs/heads/${s.branch}\n`) : fail(""),
      "HEAD...@{upstream}": s.counts ? ok(`${s.counts}\n`) : fail("no upstream"),
    };
    return answers[args.at(-1)];
  };
}

function machinesHost(list) {
  host.hc = async (args) =>
    args[1] === "ls" ? { code: 0, stdout: JSON.stringify({ machines: list }), stderr: "" } : OK;
}

function fakeCx() {
  const cx = {
    everies: [],
    timer: { every: (ms, fn) => cx.everies.push({ ms, fn }) },
    notify: vi.fn(),
    spawn: (fn) => {
      fn(cx);
    },
    stop_propagation: vi.fn(),
    write_to_clipboard: vi.fn(),
  };
  return cx;
}

function mount() {
  const cx = fakeCx();
  const view = new Spaces();
  view.init({}, cx);
  return { view, cx };
}

const flush = () => new Promise((r) => setTimeout(r, 0));
const tick = (cx) => cx.everies.find((e) => e.ms === 250).fn(cx);
const later = (ms) => vi.setSystemTime(Date.now() + ms);

const nodes = (el) => (el?.kids ? [el, ...el.kids.flatMap(nodes)] : []);
function texts(el) {
  if (typeof el === "string") {
    return [el];
  }
  return el?.kids ? el.kids.flatMap(texts) : [];
}
const idOf = (el) =>
  nodes(el).flatMap((n) => n.calls.filter(([k]) => k === "id").map((c) => c[1]))[0];
const byId = (root, id) =>
  nodes(root).find((n) => n.calls.some(([k, v]) => k === "id" && v === id));
const arg = (el, name) => el.calls.find(([k]) => k === name)?.[1];
const handler = (el, name, button) =>
  el.calls.find(([k, a]) => k === name && (button === undefined || a === button)).at(-1);
const listOf = (root) => root.kids[1].kids;
const rowIds = (root) => listOf(root).map(idOf);
const svgs = (el) =>
  nodes(el)
    .filter((n) => n.kind === "svg")
    .map((n) => n.path);
const named = (name) => host.calls.filter(([n]) => n === name).map(([, ...args]) => args);
const asked = () => named("exec").map(([, , o]) => o.cwd);
const tally = (list) =>
  Object.fromEntries([...new Set(list)].map((k) => [k, list.filter((x) => x === k).length]));
const hoverAll = (root) =>
  nodes(root)
    .flatMap((n) => n.calls.filter(([k]) => k === "hover"))
    .map(([, fn]) => fn(element("style")).calls);

let store;
beforeEach(() => {
  store = new Map();
  vi.stubGlobal("localStorage", {
    getItem: (k) => store.get(k) ?? null,
    setItem: (k, v) => store.set(k, v),
  });
  vi.useFakeTimers({ toFake: ["Date"], now: 1_000_000 });
  host.theme = T;
  host.watchFolder = () => new Promise(() => {});
  machinesHost([]);
  gitHost({});
});

afterEach(() => {
  reset();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

test("only a blocked or finished space speaks up, in its status color", () => {
  expect([glyph("blocked"), glyph("done"), glyph("working"), glyph(null)]).toStrictEqual([
    "●",
    "●",
    "·",
    "·",
  ]);
  expect(tone({ blocked: "red", unknown: "grey" }, "blocked")).toBe("red");
  expect(tone({ blocked: "red", unknown: "grey" }, "idle")).toBe("grey");
});

test("init restores machine repairs and folds, loads machines and starts both timers", async () => {
  store.set(
    "machines.reconnecting",
    JSON.stringify({ fresh: { machine: { name: "fresh" }, removed: true, busy: true } }),
  );
  store.set("machines.folded", JSON.stringify(["box"]));
  machinesHost([
    { name: "box", state: "connected" },
    { name: "tmp", temporary: true },
  ]);
  const { view, cx } = mount();
  await flush();
  expect(view.reconnecting).toStrictEqual({
    fresh: { machine: { name: "fresh" }, removed: true, busy: false },
  });
  expect([...view.folded]).toStrictEqual(["box"]);
  expect(view.machines).toStrictEqual([{ name: "box", state: "connected" }]);
  expect(cx.everies.map((e) => e.ms)).toStrictEqual([15000, 60000, 250]);
  expect(cx.notify).toHaveBeenCalledTimes(1);
  cx.everies[0].fn(cx);
  await flush();
  expect(named("hc")).toStrictEqual([[["machine", "ls", "--json"]], [["machine", "ls", "--json"]]]);
});

test("a failed or thrown machine list keeps the last one and still redraws", async () => {
  machinesHost([{ name: "box", state: "connected" }]);
  const { view, cx } = mount();
  await flush();
  host.hc = async () => ({ code: 1, stdout: "", stderr: "down" });
  cx.everies[0].fn(cx);
  await flush();
  host.hc = async () => {
    throw new Error("gone");
  };
  cx.everies[0].fn(cx);
  await flush();
  host.hc = async () => ({ code: 0, stdout: "{}", stderr: "" });
  cx.everies[0].fn(cx);
  await flush();
  expect(view.machines).toStrictEqual([]);
  expect(cx.notify).toHaveBeenCalledTimes(4);
});

test("a new place gets one git look: branch, ahead/behind and a watch on its repo", async () => {
  host.spaces = [space("a", { name: "alpha" })];
  gitHost({ "/src/a": { top: "/src/a", common: "/src/a/.git", branch: "spike", counts: "2\t1" } });
  const { view, cx } = mount();
  tick(cx);
  await flush();
  expect(named("exec")).toStrictEqual([
    [
      "git",
      ["rev-parse", "--path-format=absolute", "--show-toplevel"],
      { cwd: "/src/a", on: null },
    ],
    [
      "git",
      ["rev-parse", "--path-format=absolute", "--git-common-dir"],
      { cwd: "/src/a", on: null },
    ],
    ["git", ["symbolic-ref", "-q", "HEAD"], { cwd: "/src/a", on: null }],
    [
      "git",
      ["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
      { cwd: "/src/a", on: null },
    ],
  ]);
  expect(named("watchFolder")).toStrictEqual([["/src/a/.git"]]);
  expect(view.git).toStrictEqual({
    '[null,"/src/a"]': {
      name: "a",
      branch: "spike",
      ahead: 2,
      behind: 1,
      repo: '[null,"/src/a/.git"]',
      linked: false,
    },
  });
  const row = byId(view.render(cx), "space-a");
  expect(texts(row).slice(1)).toStrictEqual(["alpha", "a · spike", "↑2", "↓1"]);
  tick(cx);
  await flush();
  expect(asked().length).toBe(4);
});

test("asked again while looking, a place is looked at once more after the current look", async () => {
  host.spaces = [space("a")];
  gitHost({ "/src/a": { top: "/src/a", common: "/src/a/.git", branch: "main", counts: "0 0" } });
  const { cx } = mount();
  tick(cx);
  tick(cx);
  await flush();
  expect(asked()).toStrictEqual(Array(8).fill("/src/a"));
  expect(named("watchFolder")).toStrictEqual([["/src/a/.git"]]);
});

test("git's answers: detached, no upstream, bare, not a repo, unsure, away", async () => {
  host.spaces = [
    space("det"),
    space("bare"),
    space("none"),
    space("odd"),
    space("half"),
    space("far", { machine: "box" }),
  ];
  host.machines = [{ name: "box", layout: "ready" }];
  gitHost({
    "/src/det": { top: "/src/det", common: "/src/det/.git" },
    "/src/bare": { top: "/srv/tool", common: "/srv/tool.git", branch: "dev", counts: "0\t3" },
    "/src/odd": { error: "fatal: cannot change to '/src/odd'" },
    "/src/half": { top: "/src/half", commonError: "fatal: not a git repository" },
    "/src/far": { throws: true },
  });
  const { view, cx } = mount();
  tick(cx);
  await flush();
  expect(view.git).toStrictEqual({
    '[null,"/src/det"]': {
      name: "det",
      branch: "(detached)",
      ahead: 0,
      behind: 0,
      repo: '[null,"/src/det/.git"]',
      linked: false,
    },
    '[null,"/src/bare"]': {
      name: "tool",
      branch: "dev",
      ahead: 0,
      behind: 3,
      repo: '[null,"/srv/tool.git"]',
      linked: true,
    },
    '[null,"/src/none"]': null,
    '[null,"/src/odd"]': null,
    '[null,"/src/half"]': null,
    '["box","/src/far"]': null,
  });
  expect([...view.unsure]).toStrictEqual(['[null,"/src/odd"]', '["box","/src/far"]']);
  expect(named("watchFolder")).toStrictEqual([["/src/det/.git"], ["/srv/tool.git"]]);
});

test("a look that throws keeps what the place showed", async () => {
  host.spaces = [space("a")];
  gitHost({ "/src/a": { top: "/src/a", common: "/src/a/.git", branch: "dev", counts: "1 0" } });
  const { view, cx } = mount();
  tick(cx);
  await flush();
  gitHost({ "/src/a": { throws: true } });
  later(60001);
  tick(cx);
  await flush();
  expect(view.git['[null,"/src/a"]'].branch).toBe("dev");
  expect([...view.unsure]).toStrictEqual(['[null,"/src/a"]']);
});

test("a place is looked at again when a command or turn ends, with the rest of its repo", async () => {
  host.spaces = [space("a"), space("b"), space("c")];
  host.panes = [
    pane("p1", "tab-a", { status: "working" }),
    pane("p3", "tab-c", { program: { shell: false, agent: false } }),
    pane("p4", "tab-b", { terminal: null }),
    pane("p5", "tab-gone"),
  ];
  gitHost({
    "/src/a": { top: "/src/a", common: "/src/a/.git", branch: "main", counts: "0 0" },
    "/src/b": { top: "/src/b", common: "/src/a/.git", branch: "b", counts: "0 0" },
  });
  const { cx } = mount();
  tick(cx);
  await flush();
  expect(tally(asked())).toStrictEqual({ "/src/a": 4, "/src/b": 4, "/src/c": 2 });
  host.panes = [
    pane("p1", "tab-a"),
    pane("p3", "tab-c"),
    pane("p4", "tab-b", { terminal: null }),
    pane("p5", "tab-gone"),
  ];
  tick(cx);
  await flush();
  expect(tally(asked())).toStrictEqual({ "/src/a": 8, "/src/b": 8, "/src/c": 4 });
  tick(cx);
  await flush();
  expect(asked().length).toBe(20);
});

test("a space that moves to a known place has that place looked at again", async () => {
  host.spaces = [
    space("a"),
    space("b"),
    space("z", { folder: "" }),
    space("y", { tabs: undefined }),
  ];
  host.panes = [pane("p1", "tab-a", { cwd: "/src/a" })];
  gitHost({
    "/src/a": { top: "/src/a", common: "/src/a/.git", branch: "main", counts: "0 0" },
    "/src/b": { top: "/src/b", common: "/src/b/.git", branch: "main", counts: "0 0" },
  });
  const { cx } = mount();
  tick(cx);
  await flush();
  expect(tally(asked())).toStrictEqual({ "/src/a": 4, "/src/b": 4, "/src/y": 2 });
  host.panes = [pane("p1", "tab-a", { cwd: "/src/b" })];
  tick(cx);
  await flush();
  expect(tally(asked())).toStrictEqual({ "/src/a": 4, "/src/b": 8, "/src/y": 2 });
});

test("a machine that is back has its places looked at again", async () => {
  host.spaces = [space("r", { machine: "box" }), space("l")];
  host.machines = [{ name: "box", layout: "loading" }];
  gitHost({ "/src/r": { throws: true } });
  const { cx } = mount();
  tick(cx);
  await flush();
  expect(named("exec").map(([, , o]) => o.on)).toStrictEqual(["box", "box", null, null]);
  host.machines = [{ name: "box", layout: "unavailable" }];
  tick(cx);
  await flush();
  host.machines = [{ name: "box", layout: "ready" }];
  tick(cx);
  await flush();
  expect(tally(asked())).toStrictEqual({ "/src/r": 4, "/src/l": 2 });
  expect(named("watchFolder")).toStrictEqual([]);
});

test("repos in view are looked at again after a minute, only while the window is in front", async () => {
  store.set("machines.folded", JSON.stringify(["box"]));
  host.spaces = [space("a"), space("u"), space("n"), space("f", { machine: "box" })];
  gitHost({
    "/src/a": { top: "/src/a", common: "/src/a/.git", branch: "main", counts: "0 0" },
    "/src/u": { throws: true },
    "/src/f": { top: "/src/f", common: "/src/f/.git", branch: "main", counts: "0 0" },
  });
  const { cx } = mount();
  tick(cx);
  await flush();
  expect(tally(asked())).toStrictEqual({ "/src/a": 4, "/src/u": 2, "/src/n": 2, "/src/f": 4 });
  later(60001);
  host.window = { ...host.window, active: false };
  tick(cx);
  await flush();
  expect(asked().length).toBe(12);
  host.window = { ...host.window, active: true };
  tick(cx);
  await flush();
  expect(tally(asked())).toStrictEqual({ "/src/a": 8, "/src/u": 4, "/src/n": 2, "/src/f": 4 });
});

test("a change in a watched repo refreshes its places until the repo goes", async () => {
  host.spaces = [space("a")];
  const waits = [];
  host.watchFolder = () => new Promise((resolve) => waits.push(resolve));
  gitHost({ "/src/a": { top: "/src/a", common: "/src/a/.git", branch: "main", counts: "0 0" } });
  const { view, cx } = mount();
  tick(cx);
  await flush();
  waits[0]([]);
  await flush();
  expect(asked().length).toBe(8);
  gitHost({});
  waits[1]([]);
  await flush();
  expect(asked().length).toBe(10);
  expect([...view.watching]).toStrictEqual([]);
  waits[2]([]);
  await flush();
  expect(asked().length).toBe(10);
  expect(named("watchFolder").length).toBe(3);
});

test("a watch that fails is dropped and made again at the next look", async () => {
  host.spaces = [space("a")];
  host.watchFolder = async () => {
    throw new Error("no watch");
  };
  gitHost({ "/src/a": { top: "/src/a", common: "/src/a/.git", branch: "main", counts: "0 0" } });
  const { view, cx } = mount();
  tick(cx);
  await flush();
  expect([...view.watching]).toStrictEqual([]);
  later(60001);
  tick(cx);
  await flush();
  expect(named("watchFolder")).toStrictEqual([["/src/a/.git"], ["/src/a/.git"]]);
});

test("worktrees sit under their main checkout, and the order is written back once", async () => {
  host.spaces = [
    space("w1", { folder: "/src/w1" }),
    space("m"),
    space("w2"),
    space("m2", { folder: "/src/m" }),
    space("x"),
  ];
  gitHost({
    "/src/m": { top: "/src/m", common: "/src/m/.git", branch: "main", counts: "0 0" },
    "/src/w1": { top: "/src/w1", common: "/src/m/.git", branch: "w1", counts: "0 0" },
    "/src/w2": { top: "/src/w2", common: "/src/m/.git", branch: "feat", counts: "0 0" },
  });
  const { view, cx } = mount();
  tick(cx);
  await flush();
  const root = view.render(cx);
  expect(rowIds(root)).toStrictEqual(["space-m", "space-w1", "space-w2", "space-m2", "space-x"]);
  expect(named("run")).toStrictEqual([["space.move", { id: "m", index: 1 }]]);
  const [, w1, w2] = listOf(root);
  expect(svgs(w1)).toStrictEqual(["mods/workspace-chrome/branch.svg"]);
  expect([arg(w1, "h"), arg(listOf(root)[0], "h")]).toStrictEqual([28, 30]);
  expect(texts(w2)).toStrictEqual(["w2", "feat"]);
  view.render(cx);
  expect(named("run").length).toBe(1);
  cx.notify.mockClear();
  host.machines = [{ name: null, layout: "ready" }];
  later(1000);
  tick(cx);
  expect(cx.notify).toHaveBeenCalledTimes(1);
  view.render(cx);
  expect(named("run").length).toBe(2);
  later(1000);
  view.render(cx);
  expect(named("run").length).toBe(3);
  host.spaces = ["m", "w1", "w2", "m2", "x"].map((id) => host.spaces.find((s) => s.id === id));
  view.render(cx);
  expect(view.ordering).toBe(null);
  cx.notify.mockClear();
  later(1000);
  tick(cx);
  expect(cx.notify).toHaveBeenCalledTimes(0);
});

test("a space made meanwhile waits for its git, at most a moment", async () => {
  host.spaces = [space("a")];
  gitHost({ "/src/b": { hang: true } });
  const { view, cx } = mount();
  tick(cx);
  await flush();
  host.spaces = [space("a"), space("b"), space("c")];
  cx.notify.mockClear();
  tick(cx);
  expect(cx.notify).toHaveBeenCalledTimes(1);
  expect(rowIds(view.render(cx))).toStrictEqual(["space-a"]);
  await flush();
  expect(rowIds(view.render(cx))).toStrictEqual(["space-a", "space-c"]);
  later(1501);
  expect(rowIds(view.render(cx))).toStrictEqual(["space-a", "space-b", "space-c"]);
});

test("with no machines and nothing shown yet, the list says it is loading", () => {
  host.machines = [{ name: null, layout: "loading" }];
  const { view, cx } = mount();
  const root = view.render(cx);
  expect(rowIds(root)).toStrictEqual(["machine-loading-local"]);
  expect(texts(listOf(root)[0])).toStrictEqual(["Loading…"]);
  host.machines = [];
  host.spaces = [space("n")];
  expect(rowIds(view.render(cx))).toStrictEqual(["machine-loading-local"]);
  host.spaces = [];
  expect(rowIds(view.render(cx))).toStrictEqual([]);
});

test("the tail drops what the name already says, master and main", () => {
  const { view } = mount();
  const tail = (g, tree, name) => texts(view.gitTail(g, tree, name, T));
  const g = { name: "herdr", branch: "spike", ahead: 0, behind: 0 };
  expect(tail(g, null, "x")).toStrictEqual(["herdr · spike"]);
  expect(tail(g, "├─", "x")).toStrictEqual(["spike"]);
  expect(tail(g, null, "Spike")).toStrictEqual(["herdr"]);
  expect(tail({ ...g, branch: "master" }, null, "HERDR")).toStrictEqual([]);
  expect(tail({ ...g, branch: "main" }, null, "x")).toStrictEqual(["herdr"]);
  expect(tail({ name: "herdr", ahead: 0, behind: 0 }, null, "herdr")).toStrictEqual(["detached"]);
});

test("new spaces get the least used color, leavers lose theirs, home gets none", () => {
  host.shared = { "space.colors": { gone: "ansi1", a: "ansi6" } };
  host.spaces = [space("a"), space("b"), space("c"), space("home", { name: "~" })];
  const { view, cx } = mount();
  view.render(cx);
  expect(named("setShared")).toStrictEqual([
    ["space.colors", { a: "ansi6", b: "ansi3", c: "ansi4" }],
  ]);
  view.render(cx);
  expect(named("setShared").length).toBe(1);
  host.shared = { "space.colors": { gone: "ansi1" } };
  host.spaces = [];
  view.render(cx);
  expect(named("setShared").at(-1)).toStrictEqual(["space.colors", {}]);
  host.shared = {};
  view.render(cx);
  expect(named("setShared").length).toBe(2);
});

test("the header adds a space, and letting go anywhere ends a drag", () => {
  const { view, cx } = mount();
  view.pressed = "a";
  const root = view.render(cx);
  const header = root.kids[0];
  expect(texts(header)).toStrictEqual(["Spaces"]);
  expect(hoverAll(header)).toStrictEqual([
    [
      ["bg", "hover"],
      ["text_color", "foreground"],
    ],
  ]);
  handler(byId(root, "space-new"), "on_click")();
  expect(named("run")).toStrictEqual([["space.new"]]);
  handler(root.kids[1], "on_mouse_up", "left")({}, cx);
  expect(view.pressed).toBe(null);
  expect(cx.notify).toHaveBeenCalledTimes(1);
});

test("drag a row onto another to move it; click to select; right-click for its menu", () => {
  host.spaces = [
    space("a", { active: true }),
    space("b"),
    space("c"),
    space("r", { machine: "box" }),
  ];
  host.machines = [
    { name: null, layout: "ready" },
    { name: "box", layout: "ready" },
  ];
  const { view, cx } = mount();
  const row = (id) => byId(view.render(cx), `space-${id}`);
  expect(hoverAll(row("a"))).toStrictEqual([]);
  expect(hoverAll(row("b"))).toStrictEqual([[["bg", "hover"]]]);
  expect(arg(row("a"), "bg")).toBe("selected");
  handler(row("a"), "on_mouse_move")({}, cx);
  expect(cx.notify).toHaveBeenCalledTimes(0);
  handler(row("a"), "on_mouse_down", "left")();
  handler(row("c"), "on_mouse_move")({}, cx);
  expect(cx.notify).toHaveBeenCalledTimes(1);
  expect(arg(row("a"), "opacity")).toBe(0.5);
  const line = row("c").kids[0];
  expect([arg(line, "bg"), arg(line, "bottom"), arg(line, "top")]).toStrictEqual([
    "accent",
    0,
    undefined,
  ]);
  handler(row("c"), "on_mouse_move")({}, cx);
  expect(cx.notify).toHaveBeenCalledTimes(1);
  handler(row("r"), "on_mouse_move")({}, cx);
  expect(view.over).toBe(null);
  handler(row("c"), "on_mouse_move")({}, cx);
  handler(row("c"), "on_mouse_up", "left")({}, cx);
  expect(named("run")).toStrictEqual([["space.move", { id: "a", index: 3 }]]);
  expect([view.pressed, view.over]).toStrictEqual([null, null]);
  handler(row("c"), "on_mouse_down", "left")();
  handler(row("a"), "on_mouse_move")({}, cx);
  expect(arg(row("a").kids[0], "top")).toBe(0);
  const target = row("a");
  host.spaces = host.spaces.filter((s) => s.id !== "a");
  handler(target, "on_mouse_up", "left")({}, cx);
  expect(named("run").length).toBe(1);
  handler(row("b"), "on_mouse_down", "left")();
  handler(row("b"), "on_mouse_up", "left")({}, cx);
  handler(row("b"), "on_mouse_down", "left")();
  handler(row("c"), "on_mouse_move")({}, cx);
  handler(row("r"), "on_mouse_up", "left")({}, cx);
  handler(row("b"), "on_mouse_down", "right")();
  expect(named("run").slice(1)).toStrictEqual([["space.select", { id: "b" }]]);
  expect(named("openOverlay")).toStrictEqual([
    [
      {
        mod: "menu",
        kind: "popup",
        width: 200,
        height: 260,
        props: { tab: "space", target: { id: "b" } },
      },
    ],
  ]);
});

test("each machine gets a header with its state, its spaces or a way to make one", async () => {
  machinesHost([
    { name: "box", state: "connected" },
    { name: "sleepy", state: "unreachable" },
    { name: "fresh", state: "unreachable", why: REBUILT, target: "fresh.lan" },
    { name: "lock", state: "needs_login" },
    { name: "load", state: "connected" },
    { name: "warm", state: "connected" },
  ]);
  host.machines = [
    { name: null, layout: "ready" },
    { name: "box", layout: "ready" },
    { name: "warm", layout: "loading" },
  ];
  host.spaces = [space("a"), space("b", { machine: "box" }), space("g", { machine: "ghost" })];
  const { view, cx } = mount();
  await flush();
  const root = view.render(cx);
  expect(rowIds(root)).toStrictEqual([
    "machine-local",
    "space-a",
    "machine-box",
    "space-b",
    "machine-fresh",
    "machine-ghost",
    "space-g",
    "machine-load",
    "machine-loading-load",
    "machine-lock",
    "machine-login-copy-lock",
    "machine-new-lock",
    "machine-sleepy",
    "machine-new-sleepy",
    "machine-warm",
    "machine-loading-warm",
  ]);
  const head = (m) => byId(view.render(cx), `machine-${m}`);
  expect(texts(head("local"))).toStrictEqual(["This computer", "+"]);
  expect(texts(head("box"))).toStrictEqual(["box", "+"]);
  expect(texts(head("fresh"))).toStrictEqual(["fresh", "rebuilt ·", "Connect"]);
  expect(texts(head("ghost"))).toStrictEqual(["ghost", "asleep"]);
  expect(texts(head("lock"))).toStrictEqual(["lock"]);
  expect(texts(head("sleepy"))).toStrictEqual(["sleepy", "unreachable"]);
  expect(svgs(head("local"))).toStrictEqual([
    "mods/workspace-chrome/monitor.svg",
    "mods/workspace-chrome/chevron-down.svg",
  ]);
  expect(arg(byId(head("fresh"), "machine-connect-fresh"), "tooltip")).toBe(
    "This machine was rebuilt or replaced. Click to connect to the new one.",
  );
  expect(texts(byId(root, "machine-new-sleepy"))).toStrictEqual(["New space there"]);
  expect(texts(listOf(root)[10])).toStrictEqual([
    "needs a login: run",
    "hc machine login lock",
    "Copy",
  ]);
  expect(hoverAll(root).length).toBe(20);

  handler(byId(root, "machine-add-box"), "on_click")({}, cx);
  handler(byId(root, "machine-add-local"), "on_click")({}, cx);
  handler(byId(root, "machine-new-sleepy"), "on_click")();
  expect(named("run")).toStrictEqual([
    ["space.new", { machine: "box" }],
    ["space.new", { machine: "" }],
    ["space.new", { machine: "sleepy" }],
  ]);
  expect(cx.stop_propagation).toHaveBeenCalledTimes(2);

  handler(byId(root, "machine-login-copy-lock"), "on_click")({}, cx);
  expect(cx.write_to_clipboard).toHaveBeenCalledWith("hc machine login lock");
  expect(texts(byId(view.render(cx), "machine-login-copy-lock"))).toStrictEqual(["Copied"]);

  handler(head("box"), "on_click")({}, cx);
  expect(store.get("machines.folded")).toBe('["box"]');
  expect(rowIds(view.render(cx)).slice(2, 4)).toStrictEqual(["machine-box", "machine-fresh"]);
  expect(svgs(head("box"))[1]).toBe("mods/workspace-chrome/chevron-right.svg");
  handler(head("box"), "on_click")({}, cx);
  expect(store.get("machines.folded")).toBe("[]");
});

test("on the rail, machine headers are icons with a state dot and a tooltip", async () => {
  machinesHost([
    { name: "box", state: "connected" },
    { name: "fresh", state: "unreachable", why: REBUILT },
    { name: "lock", state: "needs_login" },
  ]);
  host.layout = { id: "root", children: [{ id: "bar" }, { id: "sidebar", size: { px: 100 } }] };
  host.machines = [
    { name: null, layout: "loading" },
    { name: "box", layout: "ready" },
  ];
  host.spaces = [
    space("b", { machine: "box", active: true, status: "blocked" }),
    space("g", { machine: "ghost" }),
  ];
  const { view, cx } = mount();
  await flush();
  const root = view.render(cx);
  expect(texts(root.kids[0])).toStrictEqual([]);
  expect(rowIds(root)).toStrictEqual([
    "machine-local",
    "machine-loading-local",
    "machine-box",
    "space-b",
    "machine-fresh",
    "machine-new-fresh",
    "machine-ghost",
    "space-g",
    "machine-lock",
    "machine-new-lock",
  ]);
  const head = (m) => byId(view.render(cx), `machine-${m}`);
  expect(
    ["local", "box", "fresh", "ghost", "lock"].map((m) => arg(head(m), "tooltip")),
  ).toStrictEqual([
    "This computer",
    "box · connected",
    "fresh · unreachable",
    "ghost · asleep",
    "lock needs a login: run hc machine login lock. Click for Copy.",
  ]);
  const badges = (m) =>
    nodes(head(m))
      .filter((n) => n.calls.some(([k]) => k === "absolute"))
      .map((n) => arg(n, "bg"));
  expect([badges("local"), badges("box"), badges("ghost")]).toStrictEqual([[], ["ansi2"], ["dim"]]);
  expect(texts(byId(root, "machine-loading-local"))).toStrictEqual(["…"]);
  expect(arg(byId(root, "machine-new-fresh"), "tooltip")).toBe("New space on fresh");
  const b = byId(root, "space-b");
  expect(texts(b)).toStrictEqual(["1", "●"]);
  expect(b.kids.map((k) => arg(k, "text_color"))).toStrictEqual(["foreground", "blocked"]);
  expect(texts(byId(root, "space-g"))).toStrictEqual(["2", "·"]);
  expect(hoverAll(byId(root, "space-g"))).toStrictEqual([[["bg", "hover"]]]);
  handler(b, "on_click")();
  handler(head("lock"), "on_click")({}, cx);
  handler(head("box"), "on_click")({}, cx);
  expect(named("run")).toStrictEqual([
    ["space.select", { id: "b" }],
    ["app.settings", { section: "machines" }],
  ]);
  expect(store.get("machines.folded")).toBe('["box"]');
});

test("connect re-adds a rebuilt machine with its options and forgets the repair", async () => {
  store.set("machines.folded", JSON.stringify(["fresh"]));
  const machine = {
    name: "fresh",
    state: "unreachable",
    why: REBUILT,
    target: "fresh.lan",
    via: "jump",
    session: "s1",
    temporary: false,
  };
  machinesHost([machine]);
  const { view, cx } = mount();
  await flush();
  const connect = handler(byId(view.render(cx), "machine-connect-fresh"), "on_click");
  host.hc = () => new Promise(() => {});
  connect({}, cx);
  expect(texts(byId(view.render(cx), "machine-connect-fresh"))).toStrictEqual(["Connecting…"]);
  connect({}, cx);
  expect(named("hc").length).toBe(2);
  machinesHost([]);
  view.reconnecting.fresh.busy = false;
  view.reconnecting.fresh.machine = { ...machine, temporary: true };
  view.reconnect(view.reconnecting.fresh.machine, cx);
  await flush();
  expect(named("hc").slice(2)).toStrictEqual([
    [["machine", "rm", "fresh", "--json"]],
    [
      [
        "machine",
        "add",
        "--via",
        "fresh.lan",
        "--name",
        "fresh",
        "--session",
        "s1",
        "--temp",
        "--json",
      ],
    ],
    [["machine", "ls", "--json"]],
  ]);
  expect(cx.stop_propagation).toHaveBeenCalledTimes(2);
  expect(view.reconnecting).toStrictEqual({});
  expect(store.get("machines.reconnecting")).toBe("{}");
  expect(store.get("machines.folded")).toBe("[]");
});

test("a failed connect shows why and can be tried again without removing twice", async () => {
  const machine = { name: "fresh", state: "unreachable", why: REBUILT, target: "fresh.lan" };
  const replies = [
    { code: 1, stdout: "", stderr: "" },
    { code: 0, stdout: "", stderr: "" },
    { code: 1, stdout: '{"error":{"message":"nope"}}', stderr: "" },
    { code: 1, stdout: "", stderr: "boom" },
    { code: 1, stdout: "not json", stderr: "" },
  ];
  const { view, cx } = mount();
  host.hc = async (args) =>
    args[1] === "ls"
      ? { code: 0, stdout: JSON.stringify({ machines: [] }), stderr: "" }
      : replies.shift();
  const attempt = async () => {
    view.reconnect(machine, cx);
    await flush();
    return view.reconnecting.fresh.error;
  };
  expect(await attempt()).toBe("Couldn't connect.");
  expect(await attempt()).toBe("nope");
  expect(await attempt()).toBe("boom");
  expect(await attempt()).toBe("not json");
  host.hc = async () => {
    throw "plain";
  };
  expect(await attempt()).toBe("plain");
  expect(named("hc").map(([args]) => args.slice(0, 3).join(" "))).toStrictEqual([
    "machine ls --json",
    "machine rm fresh",
    "machine ls --json",
    "machine rm fresh",
    "machine add fresh.lan",
    "machine ls --json",
    "machine add fresh.lan",
    "machine ls --json",
    "machine add fresh.lan",
    "machine ls --json",
    "machine add fresh.lan",
    "machine ls --json",
  ]);
  const stored = JSON.parse(store.get("machines.reconnecting"));
  expect(stored.fresh).toStrictEqual({ machine, removed: true, busy: false, error: "plain" });
  const head = byId(view.render(cx), "machine-fresh");
  expect(texts(head)).toStrictEqual(["fresh", "plain", "Connect"]);
  const button = byId(head, "machine-connect-fresh");
  expect([arg(button, "text_color"), arg(button, "tooltip")]).toStrictEqual(["blocked", "plain"]);
});

const ENGINE = (pid, eng, type) =>
  `\\\\PC\\GPU Engine(pid_${pid}_luid_0x00000000_0x0000B2B1_phys_0_eng_${eng}_engtype_${type})\\Utilization Percentage`;
const TYPEPERF = [
  `"(PDH-CSV 4.0)","${[
    ENGINE(1, 0, "3D"),
    ENGINE(1, 3, "Compute 1"),
    ENGINE(2, 3, "Compute 1"),
    ENGINE(2, 1, "Copy"),
  ].join('","')}"`,
  '"11/14/2026 17:28:15.267","12.400000","20.100000","22.600000","30.000000"',
  "Exiting, please wait...",
].join("\r\n");

test("the busiest engine, summed over its processes, is the load, capped at 100", () => {
  expect(parseGpuPercent(TYPEPERF)).toBe(43);
  expect(
    parseGpuPercent(
      `"(PDH-CSV 4.0)","${ENGINE(1, 3, "Compute 1")}","${ENGINE(2, 3, "Compute 1")}"\n"1","80","70"`,
    ),
  ).toBe(100);
  expect(parseGpuPercent('"(PDH-CSV 4.0)","odd counter"\n"1","7"')).toBe(7);
  expect(parseGpuPercent("Error: no valid counters.")).toBeNull();
});

test("memory in use is the part of the total not available", () => {
  expect(parseMemoryPercent("33482480 25063216\r\n")).toBe(25);
  expect(parseMemoryPercent("")).toBeNull();
  expect(parseMemoryPercent("0 0")).toBeNull();
});

const windowsHost =
  (gpu, memory) =>
  async (cmd, _args, { on }) => {
    if (on === "linux") {
      throw new Error(`${cmd}: not found`);
    }
    return cmd === "typeperf" ? gpu() : memory();
  };
const said = (stdout) => () => Promise.resolve({ code: 0, stdout, stderr: "" });

test("a connected Windows machine shows its GPU and memory load beside its icon as soon as it is listed", async () => {
  machinesHost([
    { name: "win", state: "connected" },
    { name: "linux", state: "connected" },
    { name: "away", state: "unreachable" },
  ]);
  host.machines = [{ name: null, layout: "ready" }];
  host.exec = windowsHost(said(TYPEPERF), said("100 40"));
  const { view, cx } = mount();
  await flush();
  const asks = () => named("exec").map(([cmd, , o]) => [cmd, o.on]);
  expect(asks()).toStrictEqual([
    ["typeperf", "win"],
    ["powershell", "win"],
    ["typeperf", "linux"],
    ["powershell", "linux"],
  ]);
  expect(view.load).toStrictEqual({
    win: { gpu: 43, memory: 60 },
    linux: { gpu: null, memory: null },
  });
  const win = texts(byId(view.render(cx), "machine-win"));
  expect(win.slice(win.indexOf("🎮 43%"), win.indexOf("🎮 43%") + 2)).toStrictEqual([
    "🎮 43%",
    "🐏 60%",
  ]);
  expect(texts(byId(view.render(cx), "machine-linux")).join()).not.toMatch(/🎮|🐏/);
  // The rail has no room for them.
  host.layout = { id: "root", children: [{ id: "sidebar", size: { px: 60 } }] };
  expect(texts(byId(view.render(cx), "machine-win")).join()).not.toMatch(/🎮|🐏/);

  // A new machine list asks only machines not read yet.
  host.calls.length = 0;
  cx.everies[0].fn(cx);
  await flush();
  expect(asks()).toStrictEqual([]);

  // The minute asks all again, failed ones too; an unchanged load doesn't redraw.
  cx.notify.mockClear();
  host.calls.length = 0;
  const loadTick = cx.everies.find((e) => e.ms === 60000).fn;
  loadTick(cx);
  await flush();
  expect(asks()).toHaveLength(4);
  expect(cx.notify).not.toHaveBeenCalled();

  // A read still running isn't started twice; a failed code clears that reading only.
  host.calls.length = 0;
  let answer;
  host.exec = windowsHost(() => new Promise((r) => (answer = r)), said("100 50"));
  loadTick(cx);
  loadTick(cx);
  expect(asks().filter(([, on]) => on === "win")).toHaveLength(2);
  answer({ code: 1, stdout: "", stderr: "no counter" });
  await flush();
  expect(view.load.win).toStrictEqual({ gpu: null, memory: 50 });
  expect(cx.notify).toHaveBeenCalled();

  // Leaving "connected" forgets the reading.
  view.machines = [{ name: "win", state: "unreachable" }];
  loadTick(cx);
  expect(view.load).toStrictEqual({ linux: { gpu: null, memory: null } });
});
