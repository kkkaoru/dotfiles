import type { ExecResult } from "@earendil-works/pi-coding-agent";
import type { ExecHost } from "./contracts.ts";
import { describe, expect, it, vi } from "vitest";
import {
  AgmsgClient,
  parseIdentityPairs,
  parseMembers,
  parseTeams,
  parseWhoami,
} from "./agmsg-client.ts";

type ExecMock = ReturnType<typeof vi.fn<ExecHost["exec"]>>;

const ok = (stdout: string): ExecResult => ({ code: 0, killed: false, stderr: "", stdout });

function createClient(result?: ExecResult): {
  readonly client: AgmsgClient;
  readonly exec: ExecMock;
} {
  const response: ExecResult = result ?? ok("");
  const exec: ExecMock = vi.fn<ExecHost["exec"]>(async () => response);
  return { client: new AgmsgClient(exec, "/agmsg/scripts"), exec };
}

describe("parseWhoami", () => {
  it("parses every supported identity state", () => {
    expect(parseWhoami("agent=alice teams=one,two type=pi project=/tmp/a")).toStrictEqual({
      agent: "alice",
      kind: "single",
      teams: ["one", "two"],
    });
    expect(parseWhoami("multiple=true agents=a,b teams=one,two type=pi")).toStrictEqual({
      agents: ["a", "b"],
      kind: "multiple",
      teams: ["one", "two"],
    });
    expect(parseWhoami("not_joined=true available_teams=none")).toStrictEqual({
      availableTeams: [],
      kind: "not-joined",
    });
    expect(
      parseWhoami("suggest=true agents=a teams=old type=pi available_teams=old,new"),
    ).toStrictEqual({
      agents: ["a"],
      availableTeams: ["old", "new"],
      kind: "suggestion",
      teams: ["old"],
    });
  });

  it("rejects unknown output and removes empty csv values", () => {
    expect(parseWhoami("agent=a teams=one,,two")).toMatchObject({ teams: ["one", "two"] });
    expect(() => parseWhoami("warning only")).toThrow("Unexpected whoami output");
  });
});

describe("JSON and pair parsers", () => {
  it("parses identity pairs, teams, and members", () => {
    expect(parseIdentityPairs("one\talice\ntwo\tbob\n")).toStrictEqual([
      { agent: "alice", team: "one" },
      { agent: "bob", team: "two" },
    ]);
    expect(parseTeams('{"name":"one"}\n{"name":"two"}\n')).toStrictEqual(["one", "two"]);
    expect(
      parseMembers(
        '{"name":"alice","types":["pi",2],"project":"/tmp"}\n{"name":"bob","types":[]}\n',
      ),
    ).toStrictEqual([
      { name: "alice", project: "/tmp", types: ["pi"] },
      { name: "bob", types: [] },
    ]);
  });

  it("rejects malformed records", () => {
    expect(() => parseIdentityPairs("missing-tab")).toThrow("Invalid identity");
    expect(() => parseTeams('{"name":2}')).toThrow("Invalid team");
    expect(() => parseMembers('{"types":[]}')).toThrow("Invalid member");
    expect(() => parseMembers("not-json")).toThrow();
  });
});

describe("AgmsgClient", () => {
  it("maps all public operations to stable agmsg scripts", async () => {
    const { client, exec } = createClient();
    exec.mockImplementation(async (_command: string, args: string[]) => {
      if (args[0]?.endsWith("whoami.sh") === true) {
        return ok("agent=alice teams=one type=pi\n");
      }
      if (args[0]?.endsWith("identities.sh") === true) {
        return ok("one\talice\n");
      }
      if (args[0]?.endsWith("api.sh") === true) {
        return ok('{"name":"one"}\n');
      }
      return ok("done\n");
    });
    const { signal } = new globalThis.AbortController();

    await expect(client.whoami("/project", signal)).resolves.toMatchObject({ agent: "alice" });
    await client.identities("/project", signal);
    await client.inbox({ agent: "alice", quiet: true, signal, team: "one" });
    await client.inbox({ agent: "alice", quiet: false, team: "one" });
    await client.send({ from: "alice", message: "hello", signal, team: "one", to: "bob" });
    await client.history({ agent: "alice", limit: 12, signal, team: "one" });
    await client.team("one", signal);
    await client.listTeams(signal);
    await client.join({ agent: "alice", project: "/project", signal, team: "one" });
    await client.leave({ agent: "alice", signal, team: "one" });
    await client.version(signal);

    const calls: Parameters<ExecHost["exec"]>[] = exec.mock.calls;
    expect(calls.map((call) => call[1]?.[0])).toStrictEqual([
      "/agmsg/scripts/whoami.sh",
      "/agmsg/scripts/identities.sh",
      "/agmsg/scripts/inbox.sh",
      "/agmsg/scripts/inbox.sh",
      "/agmsg/scripts/send.sh",
      "/agmsg/scripts/history.sh",
      "/agmsg/scripts/team.sh",
      "/agmsg/scripts/api.sh",
      "/agmsg/scripts/join.sh",
      "/agmsg/scripts/leave.sh",
      "/agmsg/scripts/version.sh",
    ]);
    expect(calls[2]?.[1]).toStrictEqual(["/agmsg/scripts/inbox.sh", "one", "alice", "--quiet"]);
    expect(calls[3]?.[1]).toStrictEqual(["/agmsg/scripts/inbox.sh", "one", "alice"]);
    expect(calls[0]?.[2]).toMatchObject({ cwd: "/project", signal, timeout: 30_000 });
  });

  it("parses members through api.sh", async () => {
    const { client, exec } = createClient(ok('{"name":"bob","types":["codex"]}\n'));
    await expect(client.members("one")).resolves.toStrictEqual([{ name: "bob", types: ["codex"] }]);
    expect(exec).toHaveBeenCalledWith(
      "bash",
      ["/agmsg/scripts/api.sh", "get", "teams", "one", "members"],
      { timeout: 30_000 },
    );
  });

  it("reports stderr, stdout, and exit status failures", async () => {
    const stderrClient = createClient({
      code: 2,
      killed: false,
      stderr: "bad",
      stdout: "ignored",
    }).client;
    await expect(stderrClient.version()).rejects.toThrow("version.sh failed: bad");

    const stdoutClient = createClient({
      code: 3,
      killed: false,
      stderr: "",
      stdout: "oops",
    }).client;
    await expect(stdoutClient.version()).rejects.toThrow("version.sh failed: oops");

    const statusClient = createClient({ code: 4, killed: true, stderr: "", stdout: "" }).client;
    await expect(statusClient.version()).rejects.toThrow("version.sh failed: exit 4");
  });

  it("constructs a client from an execution host", () => {
    const host: ExecHost = { exec: vi.fn(async () => ok("")) };
    expect(AgmsgClient.fromHost(host)).toBeInstanceOf(AgmsgClient);
  });
});
