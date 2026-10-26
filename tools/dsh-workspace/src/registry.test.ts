import { expect, test } from "vitest";
import { findWorkspace } from "./registry.ts";

const REGISTRY = JSON.stringify({
  unit: { name: "workspace", version: 2 },
  global: { workspaceIds: ["alpha"] },
  tables: { workspaces: { alpha: { path: "/work/repo", title: "repo" } } },
});

test("finds the workspace registered for a path", () => {
  expect(findWorkspace(REGISTRY, "/work/repo")).toStrictEqual({
    workspaceId: "alpha",
    path: "/work/repo",
  });
});

test("returns undefined for an unregistered path", () => {
  expect(findWorkspace(REGISTRY, "/work/other")).toBeUndefined();
});

test("returns undefined for malformed JSON", () => {
  expect(findWorkspace("not json", "/work/repo")).toBeUndefined();
});

test("returns undefined without a tables field", () => {
  expect(findWorkspace(JSON.stringify({ global: {} }), "/work/repo")).toBeUndefined();
});

test("returns undefined when tables is not an object", () => {
  expect(findWorkspace(JSON.stringify({ tables: 3 }), "/work/repo")).toBeUndefined();
});

test("returns undefined without a workspaces table", () => {
  expect(findWorkspace(JSON.stringify({ tables: {} }), "/work/repo")).toBeUndefined();
});

test("returns undefined when workspaces is not an object", () => {
  expect(
    findWorkspace(JSON.stringify({ tables: { workspaces: [] } }), "/work/repo"),
  ).toBeUndefined();
});

test("ignores entries that carry no path", () => {
  const registry = JSON.stringify({ tables: { workspaces: { alpha: { title: "repo" } } } });
  expect(findWorkspace(registry, "/work/repo")).toBeUndefined();
});

test("ignores scalar entries", () => {
  const registry = JSON.stringify({ tables: { workspaces: { alpha: "/work/repo" } } });
  expect(findWorkspace(registry, "/work/repo")).toBeUndefined();
});
