// Runs with Bun; all script execution is mocked.
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import {
  createAgmsgServer,
  runScript,
  type ScriptRunner,
} from "./agmsg-server.ts";

interface Connection {
  client: Client;
  server: McpServer;
}
type Callback = (error: Error | null, stdout: string, stderr: string) => void;
const execute = vi.hoisted(() =>
  vi.fn<
    (
      _command: string,
      _args: string[],
      _options: unknown,
      callback: Callback,
    ) => void
  >(),
);
vi.mock("node:child_process", () => ({
  execFile: Object.assign(execute, {
    [Symbol.for("nodejs.util.promisify.custom")]: (
      command: string,
      args: string[],
      options: unknown,
    ) =>
      new Promise<{ stdout: string; stderr: string }>((resolve, reject) => {
        execute(command, args, options, (error, stdout, stderr) => {
          if (error) reject(error);
          else resolve({ stdout, stderr });
        });
      }),
  }),
}));
const run = vi.fn<ScriptRunner>();
const connections: Connection[] = [];

beforeEach(() => {
  vi.resetAllMocks();
  run.mockResolvedValue("team\talice");
  execute.mockImplementation((_command, _args, _options, callback) =>
    callback(null, "done\n", ""),
  );
});
afterEach(async () => {
  await Promise.all(
    connections.splice(0).map(async (connection) => {
      await connection.client.close();
      await connection.server.close();
    }),
  );
});
const connect = async (): Promise<Client> => {
  const server = createAgmsgServer({
    scriptsDirectory: "/trusted/scripts",
    run,
  });
  const client = new Client({ name: "test", version: "1" });
  const [clientTransport, serverTransport] =
    InMemoryTransport.createLinkedPair();
  connections.push({ client, server });
  await server.connect(serverTransport);
  await client.connect(clientTransport);
  return client;
};

it("requires an explicit trusted scripts directory", () => {
  expect(() =>
    createAgmsgServer({ scriptsDirectory: "relative", run }),
  ).toThrow("absolute trusted");
});
it("exposes five fixed operations with accurate mutation annotations", async () => {
  const client = await connect();
  expect(
    (await client.listTools()).tools.map((tool) => [
      tool.name,
      tool.annotations?.readOnlyHint,
    ]),
  ).toStrictEqual([
    ["agmsg_whoami", true],
    ["agmsg_team", true],
    ["agmsg_history", true],
    ["agmsg_inbox", false],
    ["agmsg_send", false],
  ]);
});
it("resolves Pi identities using the caller's project rather than Executor's cwd", async () => {
  const client = await connect();
  run.mockResolvedValueOnce("agent=alice teams=team type=pi");
  const response = await client.callTool({
    name: "agmsg_whoami",
    arguments: { project: "/project with spaces" },
  });
  expect(response.isError).toBeUndefined();
  expect(run).toHaveBeenNthCalledWith(1, {
    scriptsDirectory: "/trusted/scripts",
    script: "whoami.sh",
    args: ["/project with spaces", "pi"],
  });
  expect(run).toHaveBeenNthCalledWith(2, {
    scriptsDirectory: "/trusted/scripts",
    script: "identities.sh",
    args: ["/project with spaces", "pi"],
  });
  expect(response.content).toStrictEqual([
    {
      type: "text",
      text: '{"output":"agent=alice teams=team type=pi\\nRegistered team/agent pairs:\\nteam\\talice","truncated":false}',
    },
  ]);
});
it("lists a team without accessing its files directly", async () => {
  const client = await connect();
  await client.callTool({ name: "agmsg_team", arguments: { team: "team" } });
  expect(run).toHaveBeenCalledExactlyOnceWith({
    scriptsDirectory: "/trusted/scripts",
    script: "team.sh",
    args: ["team"],
  });
});
it("validates the explicit sender before sending literal message arguments", async () => {
  const client = await connect();
  const response = await client.callTool({
    name: "agmsg_send",
    arguments: {
      project: "/project",
      team: "team",
      agent: "alice",
      to: "bob",
      message: "$(touch /tmp/nope); ' hello\nworld",
    },
  });
  expect(response.isError).toBeUndefined();
  expect(run).toHaveBeenNthCalledWith(1, {
    scriptsDirectory: "/trusted/scripts",
    script: "identities.sh",
    args: ["/project", "pi"],
  });
  expect(run).toHaveBeenNthCalledWith(2, {
    scriptsDirectory: "/trusted/scripts",
    script: "send.sh",
    args: ["team", "alice", "bob", "$(touch /tmp/nope); ' hello\nworld"],
  });
});
it("rejects another project's or unregistered sender without sending", async () => {
  const client = await connect();
  const response = await client.callTool({
    name: "agmsg_send",
    arguments: {
      project: "/project",
      team: "team",
      agent: "other",
      to: "bob",
      message: "hello",
    },
  });
  expect(response.isError).toBe(true);
  expect(run).toHaveBeenCalledOnce();
  expect(response.content).toStrictEqual([
    {
      type: "text",
      text: expect.stringMatching(/^Identity is not registered/u),
    },
  ]);
});
it("checks a registered inbox only on explicit invocation", async () => {
  const client = await connect();
  expect(run).not.toHaveBeenCalled();
  await client.callTool({
    name: "agmsg_inbox",
    arguments: { project: "/project", team: "team", agent: "alice" },
  });
  expect(run).toHaveBeenLastCalledWith({
    scriptsDirectory: "/trusted/scripts",
    script: "inbox.sh",
    args: ["team", "alice"],
  });
});
it("uses a bounded history query", async () => {
  const client = await connect();
  await client.callTool({
    name: "agmsg_history",
    arguments: { project: "/project", team: "team", agent: "alice", limit: 3 },
  });
  expect(run).toHaveBeenLastCalledWith({
    scriptsDirectory: "/trusted/scripts",
    script: "history.sh",
    args: ["team", "alice", "3"],
  });
});
it.each([
  { name: "agmsg_whoami", arguments: { project: "relative" } },
  { name: "agmsg_team", arguments: { team: "../escape" } },
  { name: "agmsg_team", arguments: { team: "a..b" } },
  {
    name: "agmsg_history",
    arguments: {
      project: "/project",
      team: "team",
      agent: "alice",
      limit: 101,
    },
  },
  {
    name: "agmsg_send",
    arguments: {
      project: "/project",
      team: "team",
      agent: "alice",
      to: "bob",
      message: "",
    },
  },
])(
  "rejects invalid inputs before executing scripts: $name $arguments",
  async (request) => {
    const client = await connect();
    expect((await client.callTool(request)).isError).toBe(true);
    expect(run).not.toHaveBeenCalled();
  },
);
it("bounds output and reports truncation", async () => {
  const client = await connect();
  run.mockResolvedValue("x".repeat(25000));
  expect(
    (await client.callTool({ name: "agmsg_team", arguments: { team: "team" } }))
      .content,
  ).toStrictEqual([
    {
      type: "text",
      text: expect.stringMatching(
        /^\{"output":"x{24000}","truncated":true\}$/u,
      ),
    },
  ]);
});
it("reports script failures without crashing or treating them as successful sends", async () => {
  const client = await connect();
  run.mockRejectedValue(new Error("script timed out"));
  expect(
    await client.callTool({ name: "agmsg_team", arguments: { team: "team" } }),
  ).toStrictEqual({
    isError: true,
    content: [{ type: "text", text: "script timed out" }],
  });
  run.mockRejectedValue("failure");
  expect(
    await client.callTool({ name: "agmsg_team", arguments: { team: "team" } }),
  ).toStrictEqual({
    isError: true,
    content: [{ type: "text", text: "agmsg operation failed" }],
  });
});
it("executes Bash with separate argv, a hard deadline and a bounded buffer", async () => {
  expect(
    await runScript({
      scriptsDirectory: "/trusted/scripts",
      script: "team.sh",
      args: ["team"],
    }),
  ).toBe("done");
  expect(execute).toHaveBeenCalledWith(
    "bash",
    ["/trusted/scripts/team.sh", "team"],
    {
      timeout: 10000,
      killSignal: "SIGKILL",
      maxBuffer: 262144,
      encoding: "utf8",
    },
    expect.any(Function),
  );
});
