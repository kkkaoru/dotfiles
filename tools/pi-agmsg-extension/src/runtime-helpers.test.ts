import { describe, expect, it } from "vitest";
import { incomingInboxContent, undeliveredInbox } from "./agmsg-operations.ts";
import type { ActiveIdentity } from "./contracts.ts";
import {
  combine,
  defaultTeamName,
  errorMessage,
  firstTeam,
  parseCommand,
  parseLimit,
  parseSend,
  selectTeam,
  uniqueAgentName,
  uniqueStrings,
  uniqueTeamName,
} from "./runtime-helpers.ts";

const identity: ActiveIdentity = { agent: "alice", teams: ["one", "two"] };

describe("runtime helpers", () => {
  it("combines non-empty script output", () => {
    expect(combine(["one", "", "two"])).toBe("one\n\ntwo");
    expect(uniqueStrings(["one", "two", "one", ""])).toStrictEqual(["one", "two", ""]);
  });

  it("parses commands and send arguments", () => {
    expect(parseCommand("  history   10 ")).toStrictEqual({ command: "history", rest: "10" });
    expect(parseCommand("team")).toStrictEqual({ command: "team", rest: "" });
    expect(parseSend("bob hello there")).toStrictEqual({ message: "hello there", to: "bob" });
    expect(() => parseSend("bob")).toThrow("Usage");
    expect(() => parseSend(" ")).toThrow("Usage");
  });

  it("derives project defaults and collision-free random agent names", () => {
    expect(defaultTeamName("/Users/kkk4oru/ghq/github.com/kkkaoru/dotfiles")).toBe("dotfiles");
    expect(defaultTeamName("/")).toBe("project");
    expect(uniqueTeamName("dotfiles", [])).toBe("dotfiles");
    expect(uniqueTeamName("dotfiles", ["dotfiles", "dotfiles-2"])).toBe("dotfiles-3");
    expect(uniqueAgentName([], () => "12345678-1234-abcd-9999-123456789abc")).toBe(
      "pi-123456781234",
    );
    expect(
      uniqueAgentName(["pi-123456781234", "pi-123456781234-2"], () => "12345678-1234-abcd"),
    ).toBe("pi-123456781234-3");
  });

  it("selects and validates teams", () => {
    expect(firstTeam(identity)).toBe("one");
    expect(() => firstTeam({ agent: "nobody", teams: [] })).toThrow("has no teams");
    expect(selectTeam(identity, undefined)).toBe(identity);
    expect(selectTeam(identity, "two")).toStrictEqual({ agent: "alice", teams: ["two"] });
    expect(() => selectTeam(identity, "missing")).toThrow("not in team");
  });

  it("validates history limits", () => {
    expect(parseLimit("")).toBe(20);
    expect(parseLimit("100")).toBe(100);
    expect(() => parseLimit("0")).toThrow("integer from 1 to 100");
    expect(() => parseLimit("1.5")).toThrow("integer from 1 to 100");
    expect(() => parseLimit("101")).toThrow("integer from 1 to 100");
  });

  it("normalizes error values", () => {
    expect(errorMessage(new Error("bad"))).toBe("bad");
    expect(errorMessage("bad")).toBe("bad");
  });

  it("keeps inbox output until a matching session entry exists", () => {
    const pending: readonly string[] = ["first", "second"];
    expect(undeliveredInbox(pending, [])).toStrictEqual(pending);
    expect(
      undeliveredInbox(pending, [
        { content: incomingInboxContent("first"), customType: "agmsg-inbox" },
        { content: "other", customType: "agmsg-sent" },
      ]),
    ).toStrictEqual(["second"]);
    expect(
      undeliveredInbox(["first"], [{ content: "first", customType: "agmsg-inbox" }]),
    ).toStrictEqual([]);
  });
});
