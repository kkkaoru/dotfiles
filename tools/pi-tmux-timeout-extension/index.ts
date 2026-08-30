// This TypeScript file is executed with Bun.
import { type Static, Type, type TSchema } from "typebox";
import { ArtifactCleaner, type ArtifactCleanerOptions } from "./src/cleanup.ts";
import { ActiveTaskDisplay, recoverActiveTaskDisplayState } from "./src/active-display.ts";
import { CompletionDelivery, type CompletionDeliveryContext } from "./src/delivery.ts";
import { type ActiveDisplayCommandHost, registerDisplayCommand } from "./src/display-command.ts";
import {
  markCompletionDelivered,
  persistTmuxLaunch,
  recoverSessionTmuxLaunches,
  type RecoveryOptions,
} from "./src/persistence.ts";
import type { Completion } from "./src/waiter.ts";
import {
  type MutableBashInput,
  TMUX_LAUNCH_TIMEOUT_MILLISECONDS,
  type TmuxLaunch,
  TmuxRuntime,
  type TmuxRuntimeOptions,
} from "./src/tmux.ts";

export { CompletionDelivery, wakePiOnCompletion } from "./src/delivery.ts";

const tmuxExecSchema = Type.Object({
  command: Type.String({
    description: "Long-running shell command to start in detached tmux",
    minLength: 1,
  }),
  estimatedDurationSeconds: Type.Optional(
    Type.Integer({
      description: "Estimated duration in seconds for expected completion time",
      maximum: 604_800,
      minimum: 1,
    }),
  ),
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

export interface TmuxExtensionRuntimeOptions {
  readonly cleanup?: ArtifactCleanerOptions;
  readonly events?: NonNullable<TmuxRuntimeOptions["events"]>;
  readonly operations?: NonNullable<TmuxRuntimeOptions["operations"]>;
  readonly recovery?: false | Omit<RecoveryOptions, "sessionNamespace">;
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

type TmuxLifecycleEvent =
  | "agent_settled"
  | "agent_start"
  | "session_before_compact"
  | "session_compact"
  | "session_compact_failed"
  | "session_shutdown"
  | "session_start"
  | "tool_call"
  | "tool_result";

export interface TmuxExtensionHost extends ActiveDisplayCommandHost {
  readonly exec: (
    command: string,
    args: readonly string[],
    options?: ExecOptions,
  ) => Promise<ExecResult>;
  readonly on: (
    event: TmuxLifecycleEvent,
    handler: (event: unknown, context?: CompletionDeliveryContext) => void,
  ) => void;
  readonly registerTool: (definition: TmuxToolDefinition) => void;
  readonly sendUserMessage: (content: string) => void;
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

function resultText(launch: TmuxLaunch): string {
  return [
    "Started detached tmux command.",
    `tmux session: ${launch.sessionName}`,
    `log: ${launch.logPath}`,
    `exit status: ${launch.statusPath}`,
  ].join("\n");
}

function registerLifecycleHandlers(input: {
  readonly activeDisplay: ActiveTaskDisplay;
  readonly cleaner: ArtifactCleaner;
  readonly delivery: CompletionDelivery;
  readonly host: TmuxExtensionHost;
  readonly recovery: false | Omit<RecoveryOptions, "sessionNamespace"> | undefined;
  readonly rewriter: AutomaticTmuxRewriter;
  readonly runtime: TmuxRuntime;
}): void {
  input.host.on("tool_call", (event: unknown): void => input.rewriter.toolCall(event));
  input.host.on("tool_result", (event: unknown): void => input.rewriter.toolResult(event));
  input.host.on("session_start", (_event: unknown, context?: CompletionDeliveryContext): void => {
    if (context === undefined) {
      return;
    }
    const { sessionManager } = context;
    if (sessionManager === undefined) {
      return;
    }
    input.activeDisplay.restore(recoverActiveTaskDisplayState(sessionManager.getEntries()));
    input.activeDisplay.setContext(context);
    input.delivery.setContext(context);
    input.delivery.beforeCompaction();
    const sessionNamespace = input.runtime.startSession(sessionManager.getSessionId());
    input.runtime.restore(
      recoverSessionTmuxLaunches(
        sessionManager.getEntries(),
        sessionNamespace,
        input.recovery === false ? undefined : input.recovery?.operations,
      ),
    );
    input.delivery.afterCompaction(context);
  });
  input.host.on("agent_start", (_event: unknown, context?: CompletionDeliveryContext): void => {
    if (context !== undefined) {
      input.delivery.setContext(context);
    }
  });
  input.host.on("agent_settled", (_event: unknown, context?: CompletionDeliveryContext): void => {
    if (context !== undefined) {
      input.delivery.agentSettled(context);
    }
  });
  input.host.on(
    "session_before_compact",
    (_event: unknown, context?: CompletionDeliveryContext): void =>
      input.delivery.beforeCompaction(context),
  );
  input.host.on("session_compact", (_event: unknown, context?: CompletionDeliveryContext): void => {
    input.runtime.reconcile();
    input.delivery.afterCompaction(context);
  });
  input.host.on(
    "session_compact_failed",
    (_event: unknown, context?: CompletionDeliveryContext): void => {
      input.runtime.reconcile();
      input.delivery.afterCompaction(context);
    },
  );
  input.host.on("session_shutdown", (): void => {
    input.cleaner.stop();
    input.delivery.clear();
    input.rewriter.clear();
    input.activeDisplay.clear();
  });
}

export default function tmuxTimeoutExtension(
  host: TmuxExtensionHost,
  runtimeOptions?: TmuxExtensionRuntimeOptions,
): void {
  const activeDisplay = new ActiveTaskDisplay();
  const cleaner = new ArtifactCleaner(runtimeOptions?.cleanup);
  const recovery: false | Omit<RecoveryOptions, "sessionNamespace"> | undefined =
    runtimeOptions?.recovery;
  const delivery = new CompletionDelivery(
    host,
    recovery === false
      ? undefined
      : {
          onDelivered: (completion: Completion): void =>
            markCompletionDelivered(completion.launch, recovery?.operations),
        },
  );
  const runtime: TmuxRuntime = new TmuxRuntime({
    ...runtimeOptions,
    onActiveChange: (launches: readonly TmuxLaunch[]): void => activeDisplay.update(launches),
    onComplete: (completion: Completion): void => delivery.complete(completion),
    onTrack: (launch: TmuxLaunch): void => persistTmuxLaunch(host.appendEntry, launch),
  });
  const rewriter: AutomaticTmuxRewriter = new AutomaticTmuxRewriter(runtime);

  registerDisplayCommand(host, activeDisplay);
  host.registerTool({
    description:
      "Start a long-running shell command in a detached tmux session and return immediately. Output and exit status are written to files under the system temporary directory.",
    executionMode: "parallel",
    label: "Tmux Exec",
    name: "tmux_exec",
    parameters: tmuxExecSchema,
    promptGuidelines: [
      "Use tmux_exec instead of foreground bash for commands expected to run for at least 120 seconds or continuously watch external state.",
      "Set tmux_exec estimatedDurationSeconds to a realistic duration estimate for the command.",
      "After tmux_exec starts a command, return control promptly; pi-tmux-timeout-extension will start a named continuation when its exit-status file appears.",
    ],
    promptSnippet: "Run a long command in detached tmux without blocking pi",
    async execute(_toolCallId, params, signal) {
      const launch: TmuxLaunch = runtime.createLaunch(
        params.command,
        params.estimatedDurationSeconds,
      );
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

  registerLifecycleHandlers({
    activeDisplay,
    cleaner,
    delivery,
    host,
    recovery,
    rewriter,
    runtime,
  });
}
