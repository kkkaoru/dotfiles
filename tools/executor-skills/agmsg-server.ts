// Runs with Bun.
import { execFile } from "node:child_process";
import { isAbsolute, join } from "node:path";
import { promisify } from "node:util";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import type { CallToolResult } from "@modelcontextprotocol/sdk/types.js";
import { z } from "zod";

interface ScriptRequest {
  scriptsDirectory: string;
  script: string;
  args: string[];
}
export type ScriptRunner = (request: ScriptRequest) => Promise<string>;
interface IdentityRequest {
  project: string;
  team: string;
  agent: string;
}
interface ServerOptions {
  scriptsDirectory: string;
  run: ScriptRunner;
}

const execute = promisify(execFile);
const OUTPUT_LIMIT = 24000;
const SCRIPT_TIMEOUT_MS = 10000;
const SCRIPT_BUFFER_BYTES = 262144;
const READ_ONLY = {
  readOnlyHint: true,
  destructiveHint: false,
  openWorldHint: false,
};
const MUTATING = {
  readOnlyHint: false,
  destructiveHint: false,
  idempotentHint: false,
  openWorldHint: false,
};
const name = z
  .string()
  .regex(/^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,127}$/u)
  .refine((value) => !value.includes(".."));
const project = z
  .string()
  .min(1)
  .max(4096)
  .refine(isAbsolute, "Use the originating Pi project's absolute path");
const identitySchema = { project, team: name, agent: name };

export const runScript: ScriptRunner = async (request) => {
  const result = await execute(
    "bash",
    [join(request.scriptsDirectory, request.script), ...request.args],
    {
      timeout: SCRIPT_TIMEOUT_MS,
      killSignal: "SIGKILL",
      maxBuffer: SCRIPT_BUFFER_BYTES,
      encoding: "utf8",
    },
  );
  return result.stdout.trimEnd();
};

const result = async (
  operation: () => Promise<string>,
): Promise<CallToolResult> => {
  try {
    const output: string = await operation();
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            output: output.slice(0, OUTPUT_LIMIT),
            truncated: output.length > OUTPUT_LIMIT,
          }),
        },
      ],
    };
  } catch (error: unknown) {
    return {
      isError: true,
      content: [
        {
          type: "text",
          text: (error instanceof Error
            ? error.message
            : "agmsg operation failed"
          ).slice(0, OUTPUT_LIMIT),
        },
      ],
    };
  }
};

const registeredIdentity = async (
  options: ServerOptions,
  request: IdentityRequest,
): Promise<void> => {
  const identities: string = await options.run({
    scriptsDirectory: options.scriptsDirectory,
    script: "identities.sh",
    args: [request.project, "pi"],
  });
  if (
    !identities
      .split("\n")
      .some((line) => line === `${request.team}\t${request.agent}`)
  ) {
    throw new Error(
      "Identity is not registered for this Pi project. Call agmsg_whoami and explicitly choose the correct team and agent; no identity is inferred from Executor's process.",
    );
  }
};

export const createAgmsgServer = (options: ServerOptions): McpServer => {
  if (!isAbsolute(options.scriptsDirectory))
    throw new Error("Provide an absolute trusted agmsg scripts directory");
  const server: McpServer = new McpServer({ name: "agmsg", version: "0.1.0" });
  const run = (script: string, args: string[]): Promise<string> =>
    options.run({ scriptsDirectory: options.scriptsDirectory, script, args });
  server.registerTool(
    "agmsg_whoami",
    {
      description:
        "Resolve registered Pi identities for the originating absolute project path. A shared Executor cannot infer the active Pi session's identity. If multiple names exist, explicitly choose the existing session identity or ask the user. Does not join or change identities.",
      inputSchema: { project },
      annotations: READ_ONLY,
    },
    (request) =>
      result(
        async () =>
          `${await run("whoami.sh", [request.project, "pi"])}\nRegistered team/agent pairs:\n${await run("identities.sh", [request.project, "pi"])}`,
      ),
  );
  server.registerTool(
    "agmsg_team",
    {
      description:
        "List members of an explicit agmsg team using the official script.",
      inputSchema: { team: name },
      annotations: READ_ONLY,
    },
    (request) => result(() => run("team.sh", [request.team])),
  );
  server.registerTool(
    "agmsg_history",
    {
      description:
        "Read bounded message history for an explicitly selected registered Pi identity. Does not mark unread messages read.",
      inputSchema: {
        ...identitySchema,
        limit: z.number().int().min(1).max(100),
      },
      annotations: READ_ONLY,
    },
    (request) =>
      result(async () => {
        await registeredIdentity(options, request);
        return run("history.sh", [
          request.team,
          request.agent,
          String(request.limit),
        ]);
      }),
  );
  server.registerTool(
    "agmsg_inbox",
    {
      description:
        "Explicit one-time inbox check for a registered Pi identity. Marks messages read. Do not poll this tool: Pi's existing extension already delivers incoming messages automatically.",
      inputSchema: identitySchema,
      annotations: MUTATING,
    },
    (request) =>
      result(async () => {
        await registeredIdentity(options, request);
        return run("inbox.sh", [request.team, request.agent]);
      }),
  );
  server.registerTool(
    "agmsg_send",
    {
      description:
        "Send a message using an explicitly selected registered Pi identity and team. Verify identity with agmsg_whoami first. This sends a real message; do not retry blindly. No arbitrary shell, registration changes, or agent spawning.",
      inputSchema: {
        ...identitySchema,
        to: name,
        message: z.string().min(1).max(16000),
      },
      annotations: MUTATING,
    },
    (request) =>
      result(async () => {
        await registeredIdentity(options, request);
        return run("send.sh", [
          request.team,
          request.agent,
          request.to,
          request.message,
        ]);
      }),
  );
  return server;
};
