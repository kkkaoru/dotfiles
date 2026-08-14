import { describe, expect, it } from "vitest";
import { formatInterval, matchGroups, parseLoopCommand } from "./parser.ts";

describe("parseLoopCommand", () => {
  it("parses subcommands and a prompt-only loop", () => {
    expect(parseLoopCommand(" list ")).toStrictEqual({ kind: "list" });
    expect(parseLoopCommand("clear")).toStrictEqual({ kind: "clear" });
    expect(parseLoopCommand("pause")).toStrictEqual({ kind: "pause" });
    expect(parseLoopCommand("resume")).toStrictEqual({ kind: "resume" });
    expect(parseLoopCommand("check the build")).toStrictEqual({
      kind: "start",
      prompt: "check the build",
    });
    expect(parseLoopCommand("check every PR")).toStrictEqual({
      kind: "start",
      prompt: "check every PR",
    });
  });

  it("parses leading compact intervals and rounds seconds up", () => {
    expect(parseLoopCommand("5m check the build")).toStrictEqual({
      intervalMs: 300_000,
      kind: "start",
      prompt: "check the build",
    });
    expect(parseLoopCommand("30s")).toStrictEqual({
      intervalMs: 60_000,
      kind: "start",
      prompt: "",
    });
    expect(parseLoopCommand("2H run tests")).toStrictEqual({
      intervalMs: 7_200_000,
      kind: "start",
      prompt: "run tests",
    });
  });

  it("parses trailing natural-language intervals", () => {
    expect(parseLoopCommand("check deploy every 5 minutes")).toStrictEqual({
      intervalMs: 300_000,
      kind: "start",
      prompt: "check deploy",
    });
    expect(parseLoopCommand("check deploy every 1 day")).toStrictEqual({
      intervalMs: 86_400_000,
      kind: "start",
      prompt: "check deploy",
    });
    expect(parseLoopCommand("check deploy every 2hrs")).toStrictEqual({
      intervalMs: 7_200_000,
      kind: "start",
      prompt: "check deploy",
    });
  });

  it("rejects a malformed regular-expression capture", () => {
    const match: RegExpExecArray | null = /x/u.exec("x");
    if (match === null) {
      throw new Error("Test fixture did not match");
    }
    expect(() => matchGroups(match)).toThrow("Invalid loop interval");
  });

  it("rejects invalid numeric intervals", () => {
    expect(() => parseLoopCommand("0m nope")).toThrow(
      "Loop interval must be a positive safe integer",
    );
    expect(() => parseLoopCommand("31d nope")).toThrow("Loop interval cannot exceed 30 days");
    expect(() => parseLoopCommand("999999999999999999999d nope")).toThrow(
      "Loop interval must be a positive safe integer",
    );
  });
});

describe("formatInterval", () => {
  it("uses the largest exact unit or rounds to minutes", () => {
    expect(formatInterval(172_800_000)).toBe("2d");
    expect(formatInterval(7_200_000)).toBe("2h");
    expect(formatInterval(300_000)).toBe("5m");
    expect(formatInterval(90_001)).toBe("2m");
  });
});
