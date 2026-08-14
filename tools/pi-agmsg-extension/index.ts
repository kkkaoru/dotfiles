import { StringEnum } from "@earendil-works/pi-ai";
import {
  DEFAULT_MAX_BYTES,
  DEFAULT_MAX_LINES,
  formatSize,
  truncateHead,
} from "@earendil-works/pi-coding-agent";
import { type Static, Type, type TSchema } from "typebox";
import { AgmsgClient } from "./src/agmsg-client.ts";
import type { AgmsgActionInput, ExecHost, MessageSink, RuntimeContext } from "./src/contracts.ts";
import { AgmsgRuntime } from "./src/runtime.ts";
import { SYSTEM_SCHEDULER } from "./src/scheduler.ts";

const ACTIONS = ["inbox", "send", "history", "team", "whoami"] satisfies [
  "inbox",
  "send",
  "history",
  "team",
  "whoami",
];
const COMPLETIONS = [
  "send",
  "history",
  "team",
  "leave",
  "reconnect",
  "setup",
  "auto on",
  "auto off",
  "whoami",
  "version",
  "help",
] satisfies string[];
const actionSchema = Type.Object({
  action: StringEnum(ACTIONS),
  limit: Type.Optional(Type.Integer({ maximum: 100, minimum: 1 })),
  message: Type.Optional(Type.String({ minLength: 1 })),
  team: Type.Optional(Type.String({ minLength: 1 })),
  to: Type.Optional(Type.String({ minLength: 1 })),
}) satisfies TSchema;

type LifecycleEvent = "agent_settled" | "session_shutdown" | "session_start";

interface ToolResult {
  readonly content: readonly [{ readonly text: string; readonly type: "text" }];
  readonly details: { readonly action: AgmsgActionInput["action"]; readonly truncated: boolean };
}

interface AgmsgToolDefinition {
  readonly description: string;
  readonly executionMode: "sequential";
  readonly execute: (
    toolCallId: string,
    params: Static<typeof actionSchema>,
    signal: AbortSignal | undefined,
    onUpdate: unknown,
    context: RuntimeContext,
  ) => Promise<ToolResult>;
  readonly label: string;
  readonly name: "agmsg";
  readonly parameters: typeof actionSchema;
  readonly promptGuidelines: readonly string[];
  readonly promptSnippet: string;
}

interface Completion {
  readonly label: string;
  readonly value: string;
}

interface AgmsgCommandDefinition {
  readonly description: string;
  readonly getArgumentCompletions: (prefix: string) => readonly Completion[] | null;
  readonly handler: (args: string, context: RuntimeContext) => Promise<void>;
}

export interface AgmsgExtensionHost extends ExecHost, MessageSink {
  readonly on: (
    event: LifecycleEvent,
    handler: (event: unknown, context: RuntimeContext) => Promise<void> | void,
  ) => void;
  readonly registerCommand: (name: "agmsg", definition: AgmsgCommandDefinition) => void;
  readonly registerTool: (definition: AgmsgToolDefinition) => void;
}

interface TruncatedOutput {
  readonly text: string;
  readonly truncated: boolean;
}

function truncate(output: string): TruncatedOutput {
  const result: ReturnType<typeof truncateHead> = truncateHead(output, {
    maxBytes: DEFAULT_MAX_BYTES,
    maxLines: DEFAULT_MAX_LINES,
  });
  if (!result.truncated) {
    return { text: result.content, truncated: false };
  }
  const detail =
    `${String(result.outputLines)}/${String(result.totalLines)} lines, ${formatSize(result.outputBytes)}/${formatSize(result.totalBytes)}` satisfies string;
  return { text: `${result.content}\n\n[agmsg output truncated: ${detail}]`, truncated: true };
}

export default function agmsgExtension(host: AgmsgExtensionHost): void {
  const runtime: AgmsgRuntime = new AgmsgRuntime(
    host,
    AgmsgClient.fromHost(host),
    SYSTEM_SCHEDULER,
  );

  host.registerTool({
    description:
      "Send and receive messages between local CLI agents through agmsg. Output is limited to 50KB or 2000 lines.",
    executionMode: "sequential",
    label: "agmsg",
    name: "agmsg",
    parameters: actionSchema,
    promptGuidelines: [
      "Use agmsg for cross-agent messages; never access agmsg SQLite, team, or configuration files directly.",
      "Use agmsg action=whoami before sending when the active identity is unclear.",
      "Do not use agmsg action=inbox or shell sleep for periodic polling; the pi agmsg extension polls invisibly every five seconds. Use inbox only for an explicit one-time request.",
    ],
    promptSnippet: "Check, send, and inspect cross-agent agmsg messages",
    async execute(_toolCallId, params, signal, _onUpdate, context) {
      const input: AgmsgActionInput = params;
      const result: TruncatedOutput = truncate(
        await runtime.execute(input, { ...context, signal }),
      );
      return {
        content: [{ text: result.text, type: "text" }],
        details: { action: input.action, truncated: result.truncated },
      };
    },
  });

  host.registerCommand("agmsg", {
    description: "Check or send agmsg cross-agent messages",
    getArgumentCompletions: (prefix: string): readonly Completion[] | null => {
      const matches: readonly string[] = COMPLETIONS.filter((value: string): boolean =>
        value.startsWith(prefix),
      );
      return matches.length === 0
        ? null
        : matches.map((value: string): Completion => ({ label: value, value }));
    },
    handler: async (args: string, context: RuntimeContext): Promise<void> =>
      runtime.command(args, context),
  });

  host.on("session_start", async (_event: unknown, context: RuntimeContext): Promise<void> =>
    runtime.start(context),
  );
  host.on("agent_settled", async (_event: unknown, context: RuntimeContext): Promise<void> =>
    runtime.checkAutomatically(context),
  );
  host.on("session_shutdown", (_event: unknown, context: RuntimeContext): void =>
    runtime.stop(context),
  );
}
