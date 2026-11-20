import { afterEach, beforeEach, expect, test, vi } from "vitest";

import { element } from "./mocks/gpui-kit.js";
import { host, reset } from "./mocks/herdr.js";
import Pane from "../../.config/herdr-app/mods/pane-branch/main.js";

const T = {
  background: "#bg",
  border: "#border",
  dim: "#dim",
  foreground: "#fg",
  hover: "#hover",
  working: "#working",
  blocked: "#blocked",
  done: "#done",
};
const CLAUDE = { key: "claude", name: "Claude", agent: true };

beforeEach(() => {
  host.theme = T;
  host.props = { pane: "p1" };
});

afterEach(() => {
  reset();
  vi.restoreAllMocks();
});

const walk = (node) => (node?.calls ? [node, ...node.kids.flatMap(walk)] : []);
const idOf = (node) => node.calls.find((c) => c[0] === "id")?.[1];
const ids = (tree) => walk(tree).map(idOf).filter(Boolean);
const byId = (tree, id) => walk(tree).find((n) => idOf(n) === id);
const arg = (node, name) => node?.calls.find((c) => c[0] === name)?.[1];
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
const kinds = (tree) => walk(tree).map((n) => n.kind);

const pane = (o = {}) => ({
  pane: "p1",
  terminal: "t1",
  cwd: "/Users/me/src/app",
  branch: "feat/x",
  title: "✳ Fix bug",
  program: CLAUDE,
  status: "idle",
  ...o,
});

const newView = () => {
  const cx = { notify: vi.fn() };
  const view = new Pane();
  view.init({}, cx);
  return { view, cx };
};
const render = (view, cx) => view.render(cx);

test("a pane header reads its program, folder, branch and nothing else", () => {
  host.panes = [pane()];
  const { view, cx } = newView();
  const tree = render(view, cx);
  expect(ids(tree)).toStrictEqual(["pane-head-p1"]);
  expect(texts(tree)).toStrictEqual(["Claude", "~/src/app", "feat/x"]);
  expect(paths(tree)).toStrictEqual([]);
});

test("a pane without a branch, a checkout under home, elsewhere or nowhere", () => {
  const { view, cx } = newView();
  host.panes = [pane({ branch: null })];
  expect(texts(render(view, cx))).toStrictEqual(["Claude", "~/src/app"]);
  host.panes = [pane({ cwd: "/Users/me" })];
  expect(texts(render(view, cx))).toStrictEqual(["Claude", "~", "feat/x"]);
  host.panes = [pane({ cwd: "/opt/x" })];
  expect(texts(render(view, cx))).toStrictEqual(["Claude", "/opt/x", "feat/x"]);
  host.panes = [pane({ cwd: null })];
  expect(texts(render(view, cx))).toStrictEqual(["Claude", "", "feat/x"]);
});

test("the program's picture, a tinted glyph, or nothing", () => {
  const { view, cx } = newView();
  host.panes = [pane({ program: { ...CLAUDE, icon: "mods/x/icon.svg", tint: true } })];
  const tinted = render(view, cx);
  expect(paths(tinted)).toStrictEqual(["mods/x/icon.svg"]);
  expect(kinds(tinted)).toContain("svg");
  host.panes = [pane({ program: { ...CLAUDE, icon: "mods/x/logo.png", tint: false } })];
  const picture = walk(render(view, cx)).find((n) => n.kind === "image");
  expect(picture.path).toBe("mods/x/logo.png");
  expect(arg(picture, "w")).toBe(14);
  expect(arg(picture, "h")).toBe(14);
  host.panes = [pane({ program: { key: "claude" } })];
  expect(kinds(render(view, cx))).not.toContain("image");
});

test("a name comes from the program, the title, the program's own name, else shell", () => {
  const { view, cx } = newView();
  host.panes = [pane({ title: "✳ Fix bug", program: { agent: true, name: "" } })];
  expect(texts(render(view, cx))[0]).toBe("✳ Fix bug");
  host.panes = [pane({ title: "", program: { name: "nvim" } })];
  expect(texts(render(view, cx))[0]).toBe("nvim");
  host.panes = [pane({ title: "", program: null })];
  expect(texts(render(view, cx))[0]).toBe("shell");
});

test("the divider faces the terminal: below a top header, above a bottom one", () => {
  const { view, cx } = newView();
  host.panes = [pane()];
  const top = render(view, cx);
  expect(arg(top, "border_b")).toBe(1);
  expect(arg(top, "border_t")).toBeUndefined();
  host.layout = {
    id: "root",
    type: "row",
    children: [
      {
        id: "main",
        type: "block",
        content: {
          tab: "active",
          pane: { header: { content: { mod: "pane-branch" }, position: "bottom" } },
        },
      },
    ],
  };
  const bottom = render(view, cx);
  expect(arg(bottom, "border_t")).toBe(1);
  expect(arg(bottom, "border_b")).toBeUndefined();
});

test("the frame is found past hidden blocks and inside a stack's active child", () => {
  const { view, cx } = newView();
  host.panes = [pane()];
  host.layout = {
    id: "root",
    type: "column",
    children: [
      {
        id: "gone",
        type: "block",
        hidden: true,
        content: { tab: "active", pane: { header: { position: "bottom" } } },
      },
      {
        id: "stack",
        type: "stack",
        active: "second",
        children: [
          {
            id: "first",
            type: "block",
            content: { tab: "active", pane: { header: { position: "bottom" } } },
          },
          {
            id: "second",
            type: "block",
            content: { tab: "active", pane: { header: { position: "top" } } },
          },
        ],
      },
    ],
  };
  expect(arg(render(view, cx), "border_b")).toBe(1);
  // A block without children, and a stack whose active child is gone: the first one shown wins.
  host.layout = {
    id: "root",
    type: "column",
    children: [
      { id: "empty", type: "block" },
      {
        id: "stack",
        type: "stack",
        active: "gone",
        children: [
          {
            id: "first",
            type: "block",
            content: { tab: "active", pane: { header: { position: "bottom" } } },
          },
        ],
      },
    ],
  };
  expect(arg(render(view, cx), "border_t")).toBe(1);
});

test("a pane with no status shows no status glyph, and each status its own", () => {
  const { view, cx } = newView();
  host.panes = [pane({ status: "working" })];
  expect(paths(render(view, cx))).toStrictEqual(["mods/workspace-chrome/spinner.svg"]);
  expect(
    arg(
      walk(render(view, cx)).find((n) => n.path),
      "spin",
    ),
  ).toBe(900);
  host.panes = [pane({ status: "blocked" })];
  expect(texts(render(view, cx))).toContain("Needs you");
  host.panes = [pane({ status: "done" })];
  expect(texts(render(view, cx))).toStrictEqual(["Claude", "~/src/app", "feat/x"]);
  host.panes = [pane({ status: "idle" })];
  expect(paths(render(view, cx))).toStrictEqual([]);
});

test("hovering shows the pane's buttons, and each runs its command", () => {
  host.panes = [pane()];
  const { view, cx } = newView();
  const head = byId(render(view, cx), "pane-head-p1");
  arg(head, "on_hover")(true, cx);
  expect(cx.notify).toHaveBeenCalled();
  const tree = render(view, cx);
  expect(ids(tree)).toStrictEqual(["pane-head-p1", "ph-r-p1", "ph-d-p1", "ph-z-p1", "ph-x-p1"]);
  expect(paths(tree)).toStrictEqual([
    "mods/pane-branch/split-right.svg",
    "mods/pane-branch/split-down.svg",
    "mods/pane-branch/zoom.svg",
    "mods/pane-branch/close.svg",
  ]);
  const ecx = { stop_propagation: vi.fn(), notify: vi.fn() };
  arg(byId(tree, "ph-r-p1"), "on_click")(null, ecx);
  arg(byId(tree, "ph-x-p1"), "on_click")(null, ecx);
  expect(ecx.stop_propagation).toHaveBeenCalledTimes(2);
  expect(host.calls).toStrictEqual([
    ["run", "pane.split", { direction: "right", pane: "p1" }],
    ["run", "pane.close", { pane: "p1" }],
  ]);
});

test("a zoomed pane offers unzoom, and the header focuses the pane", () => {
  host.panes = [pane({ zoomed: true })];
  const { view, cx } = newView();
  view.hovered = true;
  const tree = render(view, cx);
  expect(paths(tree)).toContain("mods/pane-branch/unzoom.svg");
  arg(byId(tree, "ph-z-p1"), "on_click")(null, { stop_propagation: vi.fn(), notify: vi.fn() });
  arg(byId(tree, "pane-head-p1"), "on_click")();
  expect(host.calls).toStrictEqual([
    ["run", "pane.zoom", { pane: "p1" }],
    ["run", "pane.focus", { pane: "p1" }],
  ]);
});

test("a header whose pane is gone draws nothing", () => {
  host.panes = [];
  const { view, cx } = newView();
  const tree = render(view, cx);
  expect(tree.kids).toStrictEqual([]);
  expect(tree.calls).toStrictEqual([]);
  expect(element).toBeTypeOf("function");
});
