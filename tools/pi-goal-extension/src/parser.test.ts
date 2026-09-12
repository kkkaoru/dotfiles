// Runs with Bun.
import { expect, it } from "vitest";
import { parseBudget, parseGoalCommand } from "./parser.ts";

it("parses plain objectives without an invented budget", () => {
  expect(parseGoalCommand("  Verify changes\nand report  ")).toStrictEqual({
    kind: "create",
    objective: "Verify changes\nand report",
    tokenBudget: null,
  });
});
it("parses explicit budgeted objectives", () => {
  expect(parseGoalCommand("--tokens 200 Verify\nchanges")).toStrictEqual({
    kind: "create",
    objective: "Verify\nchanges",
    tokenBudget: 200,
  });
});
it("parses all user controls", () => {
  expect(parseGoalCommand("")).toStrictEqual({ kind: "status" });
  expect(parseGoalCommand("status")).toStrictEqual({ kind: "status" });
  expect(parseGoalCommand("pause")).toStrictEqual({ kind: "pause" });
  expect(parseGoalCommand("resume")).toStrictEqual({ kind: "resume" });
  expect(parseGoalCommand("clear")).toStrictEqual({ kind: "clear" });
  expect(parseGoalCommand("edit")).toStrictEqual({
    kind: "edit",
    objective: null,
  });
  expect(parseGoalCommand("edit Revised objective")).toStrictEqual({
    kind: "edit",
    objective: "Revised objective",
  });
  expect(parseGoalCommand("budget 100")).toStrictEqual({
    kind: "budget",
    tokens: 100,
  });
  expect(parseGoalCommand("budget none")).toStrictEqual({
    kind: "budget",
    tokens: null,
  });
});
it.each([
  "0",
  "-1",
  "1.5",
  "1e3",
  "Infinity",
  "9007199254740992",
  "+5",
  "01",
  "bad",
])("rejects invalid token budget %s", (text) => {
  expect(() => parseBudget(text)).toThrow(
    "Token budget must be a positive safe integer.",
  );
});
it.each(["--tokens", "--tokens 100", "--unknown task", "budget"])(
  "rejects malformed control %s",
  (text) => {
    expect(() => parseGoalCommand(text)).toThrow("/goal [");
  },
);
