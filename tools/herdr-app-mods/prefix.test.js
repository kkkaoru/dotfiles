import { afterEach, expect, test, vi } from "vitest";

import { host, reset } from "./mocks/herdr.js";
import Prefix from "../../.config/herdr-app/mods/prefix/main.js";

// A cx whose spawned task a test runs by hand.
function newPrefix() {
  const handle = { focus: vi.fn() };
  const tasks = [];
  const cx = { focus_handle: () => handle, spawn: (fn) => tasks.push(fn) };
  host.props = { overlay: "overlay-7" };
  host.theme = {
    accent: "#a",
    muted: "#m",
    popover: "#p",
    accent_foreground: "#af",
    foreground: "#f",
    dim: "#d",
  };
  const view = new Prefix();
  view.init(host.props, cx);
  return { view, handle, tasks };
}

// Presses a key on the rendered popup; returns the cx it was given.
function press(view, key, keystroke = key) {
  const onKey = view.render({}).calls.find(([k]) => k === "on_key_down")[1];
  const cx = { stop_propagation: vi.fn() };
  onKey({ key, keystroke }, cx);
  return cx;
}

const runs = () => host.calls.filter(([k]) => k === "run");

afterEach(() => {
  reset();
});

test("init focuses the popup and spawns one command listener", () => {
  const { handle, tasks } = newPrefix();
  expect(handle.focus).toHaveBeenCalledTimes(1);
  expect(tasks.length).toBe(1);
});

test("a literal command closes the popup and sends ctrl-b to the focused terminal", async () => {
  const { tasks } = newPrefix();
  host.focused = { terminal: "term-3" };
  const replies = ["literal", "other"];
  const done = Promise.withResolvers();
  host.nextCommand = () => (replies.length ? Promise.resolve(replies.shift()) : done.promise);
  tasks[0]();
  await vi.waitFor(() => expect(replies.length).toBe(0));
  expect(host.calls).toStrictEqual([
    ["nextCommand"],
    ["closeOverlay", "overlay-7"],
    ["hc", ["keys", "term-3", "ctrl+b"]],
    ["nextCommand"],
    ["nextCommand"],
  ]);
});

test("a literal command with no focused terminal only closes the popup", async () => {
  const { tasks } = newPrefix();
  const replies = ["literal"];
  const done = Promise.withResolvers();
  host.nextCommand = () => (replies.length ? Promise.resolve(replies.shift()) : done.promise);
  tasks[0]();
  await vi.waitFor(() => expect(host.calls.length).toBe(3));
  expect(host.calls).toStrictEqual([
    ["nextCommand"],
    ["closeOverlay", "overlay-7"],
    ["nextCommand"],
  ]);
});

test("a digit selects that tab", () => {
  const { view } = newPrefix();
  const cx = press(view, "3");
  expect(cx.stop_propagation).toHaveBeenCalledTimes(1);
  expect(host.calls).toStrictEqual([
    ["closeOverlay", "overlay-7"],
    ["run", "tab.select", { index: 3 }],
  ]);
});

test("a bound key runs its command with its arguments", () => {
  const { view } = newPrefix();
  press(view, "v");
  expect(runs()).toStrictEqual([["run", "pane.split", { direction: "right" }]]);
});

test("a bound key without arguments runs its command with none", () => {
  const { view } = newPrefix();
  press(view, "c");
  expect(runs()).toStrictEqual([["run", "tab.new", {}]]);
});

test("? opens help like shift-/", () => {
  const { view } = newPrefix();
  press(view, "/", "?");
  expect(runs()).toStrictEqual([["run", "app.help", {}]]);
});

test("an arrow key without a hint still focuses that way", () => {
  const { view } = newPrefix();
  press(view, "down");
  expect(runs()).toStrictEqual([["run", "pane.go", { direction: "down" }]]);
});

test("an unbound key and 0 cancel without running anything", () => {
  const { view } = newPrefix();
  press(view, "y");
  press(view, "0");
  expect(host.calls).toStrictEqual([
    ["closeOverlay", "overlay-7"],
    ["closeOverlay", "overlay-7"],
  ]);
});

test("escape cancels even though its keystroke names a command", () => {
  const { view } = newPrefix();
  press(view, "escape", "c");
  expect(host.calls).toStrictEqual([["closeOverlay", "overlay-7"]]);
});

test("a modifier alone keeps the popup open", () => {
  const { view } = newPrefix();
  const presses = ["shift", "control", "alt", "platform", "function"].map((k) => press(view, k));
  expect(presses.map((cx) => cx.stop_propagation.mock.calls.length)).toStrictEqual([0, 0, 0, 0, 0]);
  expect(host.calls).toStrictEqual([]);
});

test("the popup tracks focus and shows the hints with their keys", () => {
  const { view, handle } = newPrefix();
  const root = view.render({});
  expect(root.kind).toBe("v_flex");
  expect(root.calls.find(([k]) => k === "track_focus")).toStrictEqual(["track_focus", handle]);
  expect(root.calls.find(([k]) => k === "bg")).toStrictEqual(["bg", "#p"]);
  const [header, hints] = root.kids;
  expect(header.kids.map((d) => d.kids[0])).toStrictEqual([
    "PREFIX",
    "ctrl-b",
    undefined,
    "Esc to cancel · ctrl-b sends ctrl-b",
  ]);
  expect(hints.kids.map((row) => row.kids.map((d) => d.kids[0]))).toStrictEqual([
    ["c", "new tab"],
    ["n", "next tab"],
    ["p", "prev tab"],
    ["1-9", "select tab"],
    ["T", "rename tab"],
    ["X", "close tab"],
    ["N", "new space"],
    ["W", "rename space"],
    ["D", "close space"],
    ["v", "split right"],
    ["h", "split down"],
    ["left", "focus left"],
    ["j", "focus down"],
    ["k", "focus up"],
    ["l", "focus right"],
    ["x", "close pane"],
    ["z", "zoom"],
    ["b", "sidebar"],
    ["w", "picker"],
    ["g", "palette"],
    ["s", "settings"],
    ["?", "help"],
    ["q", "hide"],
  ]);
  expect(hints.kids[0].kids.map((d) => d.calls.find(([k]) => k === "text_color"))).toStrictEqual([
    ["text_color", "#a"],
    ["text_color", "#m"],
  ]);
});
