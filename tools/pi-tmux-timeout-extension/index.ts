// This TypeScript file is executed with Bun.
import { type Static, Type, type TSchema } from "typebox";
import type { Completion } from "./src/waiter.ts";
import {
  type MutableBashInput,
  TMUX_LAUNCH_TIMEOUT_MILLISECONDS,
  type TmuxLaunch,
  TmuxRuntime,
  type TmuxRuntimeOptions,
} from "./src/tmux.ts";

const tmuxExecSchema = Type.Object({
  command: Type.String({
    description: "Long-running shell command to start in detached tmux",
    minLength: 1,
  }),
}) satisfies TSchema;

interface ExecOptions {
  readonly signal?: AbortSignal;
  readonly timeout?: number;
}

interface ExecResult {
  readonly code: number;
  readonly stderr: string;
  readonly stdout: string;
}

interface ToolResult {
  readonly content: readonly [{ readonly text: string; readonly type: "text" }];
  readonly details: TmuxLaunch;
}

interface ExtractedBashInput {
  readonly input: MutableBashInput;
  readonly target: object;
}

export interface TmuxToolDefinition {
  readonly description: string;
  readonly executionMode: "parallel";
  readonly execute: (
    toolCallId: string,
    params: Static<typeof tmuxExecSchema>,
    signal: AbortSignal | undefined,
  ) => Promise<ToolResult>;
  readonly label: string;
  readonly name: "tmux_exec";
  readonly parameters: typeof tmuxExecSchema;
  readonly promptGuidelines: readonly string[];
  readonly promptSnippet: string;
}

export interface TmuxExtensionHost {
  readonly exec: (
    command: string,
    args: readonly string[],
    options?: ExecOptions,
  ) => Promise<ExecResult>;
  readonly on: (
    event: "session_shutdown" | "tool_call" | "tool_result",
    handler: (event: unknown) => void,
  ) => void;
  readonly registerTool: (definition: TmuxToolDefinition) => void;
  readonly sendUserMessage: (content: string, options: { readonly deliverAs: "followUp" }) => void;
}

function toolCallInput(event: unknown): unknown {
  if (typeof event !== "object" || event === null || !("toolName" in event)) {
    return undefined;
  }
  if (event.toolName !== "bash" || !("input" in event)) {
    return undefined;
  }
  return event.input;
}

function normalizedBashInput(value: unknown): ExtractedBashInput | undefined {
  if (
    typeof value !== "object" ||
    value === null ||
    !("command" in value) ||
    typeof value.command !== "string"
  ) {
    return undefined;
  }
  const timeout: unknown = "timeout" in value ? value.timeout : undefined;
  if (timeout !== undefined && typeof timeout !== "number") {
    return undefined;
  }
  const input: MutableBashInput =
    timeout === undefined ? { command: value.command } : { command: value.command, timeout };
  return { input, target: value };
}

function eventToolCallId(event: unknown): string | undefined {
  if (
    typeof event !== "object" ||
    event === null ||
    !("toolCallId" in event) ||
    typeof event.toolCallId !== "string"
  ) {
    return undefined;
  }
  return event.toolCallId;
}

function toolResultFailed(event: unknown): boolean {
  return (
    typeof event === "object" && event !== null && "isError" in event && event.isError === true
  );
}

class AutomaticTmuxRewriter {
  readonly #pending = new Map<string, TmuxLaunch>();
  readonly #runtime: TmuxRuntime;

  constructor(runtime: TmuxRuntime) {
    this.#runtime = runtime;
  }

  toolCall(event: unknown): void {
    const toolCallId: string | undefined = eventToolCallId(event);
    const extracted: ExtractedBashInput | undefined = normalizedBashInput(toolCallInput(event));
    if (toolCallId === undefined || extracted === undefined) {
      return;
    }
    const launch: TmuxLaunch | undefined = this.#runtime.rewriteLongBash(extracted.input);
    if (launch === undefined) {
      return;
    }
    Object.assign(extracted.target, extracted.input);
    this.#pending.set(toolCallId, launch);
  }

  toolResult(event: unknown): void {
    const toolCallId: string | undefined = eventToolCallId(event);
    const launch: TmuxLaunch | undefined =
      toolCallId === undefined ? undefined : this.#pending.get(toolCallId);
    if (toolCallId === undefined || launch === undefined) {
      return;
    }
    this.#pending.delete(toolCallId);
    if (!toolResultFailed(event)) {
      this.#runtime.trackLaunch(launch);
    }
  }

  clear(): void {
    this.#pending.clear();
    this.#runtime.clear();
  }
}

function submittedTimestamp(timestamp: string): string {
  return timestamp.slice(5, 16).replace("T", " ");
}

function completionPrompt(completion: Completion): string {
  const submittedAt: string = submittedTimestamp(completion.launch.submittedAt);
  const completedAt: string = new Date().toISOString().slice(11, 16);
  return `${submittedAt} → ${completedAt} | exit=${String(completion.exitCode)} | ${completion.launch.taskCommand}\n${completion.launch.logPath}`;
}

export function wakePiOnCompletion(host: TmuxExtensionHost, completion: Completion): void {
  host.sendUserMessage(completionPrompt(completion), { deliverAs: "followUp" });
}

function resultText(launch: TmuxLaunch): string {
  return [
    "Started detached tmux command.",
    `tmux session: ${launch.sessionName}`,
    `log: ${launch.logPath}`,
    `exit status: ${launch.statusPath}`,
  ].join("\n");
}

export default function tmuxTimeoutExtension(
  host: TmuxExtensionHost,
  runtimeOptions?: Pick<TmuxRuntimeOptions, "events" | "operations">,
): void {
  const runtime: TmuxRuntime = new TmuxRuntime({
    ...runtimeOptions,
    onComplete: wakePiOnCompletion.bind(undefined, host),
  });
  const rewriter: AutomaticTmuxRewriter = new AutomaticTmuxRewriter(runtime);

  host.registerTool({
    description:
      "Start a long-running shell command in a detached tmux session and return immediately. Output and exit status are written to files under the system temporary directory.",
    executionMode: "parallel",
    label: "Tmux Exec",
    name: "tmux_exec",
    parameters: tmuxExecSchema,
    promptGuidelines: [
      "Use tmux_exec instead of foreground bash for commands expected to run for at least 120 seconds or continuously watch external state.",
      "After tmux_exec starts a command, return control promptly; pi-tmux-timeout-extension will queue a follow-up immediately when its exit-status file appears.",
      "Do not call loop_wakeup or start timed polling for a command handled by tmux_exec; rely on its completion wakeup instead.",
    ],
    promptSnippet: "Run a long command in detached tmux without blocking pi",
    async execute(_toolCallId, params, signal) {
      const launch: TmuxLaunch = runtime.createLaunch(params.command);
      const options: ExecOptions =
        signal === undefined
          ? { timeout: TMUX_LAUNCH_TIMEOUT_MILLISECONDS }
          : { signal, timeout: TMUX_LAUNCH_TIMEOUT_MILLISECONDS };
      const result: ExecResult = await host.exec("sh", ["-lc", launch.command], options);
      if (result.code !== 0) {
        throw new Error(
          result.stderr.trim() || result.stdout.trim() || "Failed to start tmux command",
        );
      }
      runtime.trackLaunch(launch);
      return {
        content: [{ text: resultText(launch), type: "text" }],
        details: launch,
      };
    },
  });

  host.on("tool_call", (event: unknown): void => rewriter.toolCall(event));
  host.on("tool_result", (event: unknown): void => rewriter.toolResult(event));
  host.on("session_shutdown", (): void => rewriter.clear());
}
