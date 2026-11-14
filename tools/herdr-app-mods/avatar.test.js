import { readFileSync } from "node:fs";
import { afterEach, expect, test, vi } from "vitest";

import { host, reset } from "./mocks/herdr.js";
import * as agents from "../../.config/herdr-app/mods/agents-emoji/avatar.js";
import * as spaces from "../../.config/herdr-app/mods/spaces-emoji/avatar.js";

const AGENTS = new URL("../../.config/herdr-app/mods/agents-emoji/avatar.js", import.meta.url);
const SPACES = new URL("../../.config/herdr-app/mods/spaces-emoji/avatar.js", import.meta.url);

afterEach(() => {
  reset();
  vi.unstubAllGlobals();
});

test("the two copies are the same apart from the agents copy's first line", () => {
  const a = readFileSync(AGENTS, "utf8");
  expect(a.split("\n")[0]).toBe(
    "// Copy of spaces-emoji/avatar.js (mods cannot import from each other): keep the two the same.",
  );
  expect(a.slice(a.indexOf("\n") + 1)).toBe(readFileSync(SPACES, "utf8"));
});

// Pinned: a change gives every existing space a different animal.
test("a name always maps to the same animal in both copies", () => {
  const names = ["dotfiles", "a", "", "work", "🦀"];
  const expected = ["🐁", "🐨", "🦜", "🦋", "🦘"];
  expect(names.map(agents.spaceEmoji)).toStrictEqual(expected);
  expect(names.map(spaces.spaceEmoji)).toStrictEqual(expected);
});

test("a space's fill is its stored mark, a stored hex, a theme color or the default teal", () => {
  host.shared["space.colors"] = { s1: "ansi4", s2: "#123456", s3: "chart_1" };
  const t = { chart_1: "#c1" };
  const ids = ["s1", "s2", "s3", "s4"];
  expect(ids.map((id) => agents.spaceFill(id, t))).toStrictEqual([
    "#5b8def",
    "#123456",
    "#c1",
    "#22a99a",
  ]);
  expect(ids.map((id) => spaces.spaceFill(id, t))).toStrictEqual([
    "#5b8def",
    "#123456",
    "#c1",
    "#22a99a",
  ]);
});

test("with no stored colors every space is teal", () => {
  expect([agents.spaceFill("x", {}), spaces.spaceFill("x", {})]).toStrictEqual([
    "#22a99a",
    "#22a99a",
  ]);
});

test("an avatar is the space's emoji on its fill", () => {
  host.shared["space.colors"] = { s1: "ansi1" };
  const avatars = [agents.spaceAvatar("s1", "a", {}), spaces.spaceAvatar("s1", "a", {})];
  expect(avatars.map((a) => [a.kind, a.kids, a.calls])).toStrictEqual(
    Array.from({ length: 2 }, () => [
      "h_flex",
      ["🐨"],
      [
        ["size", 18],
        ["flex_none"],
        ["rounded", 5],
        ["bg", "#d9649a"],
        ["items_center"],
        ["justify_center"],
        ["text_size", 12],
      ],
    ]),
  );
});
