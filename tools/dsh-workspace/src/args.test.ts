import { expect, test } from "vitest";
import { helpText, lastSegment, parseArguments } from "./args.ts";

test("defaults to the current directory and its last segment", () => {
  expect(parseArguments([], "/work/repo")).toStrictEqual({
    kind: "options",
    options: { directory: "/work/repo", title: "repo" },
  });
});

test("takes a positional directory and derives its title", () => {
  expect(parseArguments(["/work/other"], "/work/repo")).toStrictEqual({
    kind: "options",
    options: { directory: "/work/other", title: "other" },
  });
});

test("accepts --title with a separate value", () => {
  expect(parseArguments(["/work/other", "--title", "My Project"], "/work/repo")).toStrictEqual({
    kind: "options",
    options: { directory: "/work/other", title: "My Project" },
  });
});

test("accepts --title=value", () => {
  expect(parseArguments(["--title=My Project"], "/work/repo")).toStrictEqual({
    kind: "options",
    options: { directory: "/work/repo", title: "My Project" },
  });
});

test("accepts -t with a separate value", () => {
  expect(parseArguments(["-t", "Short"], "/work/repo")).toStrictEqual({
    kind: "options",
    options: { directory: "/work/repo", title: "Short" },
  });
});

test("reports a title flag without its value", () => {
  expect(parseArguments(["--title"], "/work/repo")).toStrictEqual({
    kind: "usage-error",
    message: "--title needs a value",
  });
});

test("reports an unknown option", () => {
  expect(parseArguments(["--nope"], "/work/repo")).toStrictEqual({
    kind: "usage-error",
    message: "unknown option: --nope",
  });
});

test("refuses two positional directories", () => {
  expect(parseArguments(["/first", "/second"], "/work/repo")).toStrictEqual({
    kind: "usage-error",
    message: "one directory at a time",
  });
});

test("ignores the option separator", () => {
  expect(parseArguments(["--", "/work/other"], "/work/repo")).toStrictEqual({
    kind: "options",
    options: { directory: "/work/other", title: "other" },
  });
});

test("treats a lone dash as a directory", () => {
  expect(parseArguments(["-"], "/work/repo")).toStrictEqual({
    kind: "options",
    options: { directory: "-", title: "-" },
  });
});

test("reads the last segment of a trailing-slash path", () => {
  expect(lastSegment("/work/repo/")).toBe("repo");
});

test("falls back to the whole path when there is no segment", () => {
  expect(lastSegment("/")).toBe("/");
});

test("prints usage text", () => {
  expect(helpText().startsWith("usage: dsh-workspace")).toBe(true);
});
