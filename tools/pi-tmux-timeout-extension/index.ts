// This TypeScript file is executed with Bun.
import { ActivityProvider, announceTask, type ActivityBus } from "./src/goal-activity.ts";
import type { Static } from "typebox";
import { registerTmuxTool } from "./src/register-tool.ts";
import type { tmuxExecSchema } from "./src/tool-schema.ts";
import { ArtifactCleaner, type ArtifactCleanerOptions } from "./src/cleanup.ts";
import { ActiveTaskDisplay, recoverActiveTaskDisplayState } from "./src/active-display.ts";
import {
  CompletionDelivery,
  type CompletionDeliveryContext,
  type CompletionDeliveryHost,
} from "./src/delivery.ts";
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
  type TmuxLaunch,
  TmuxRuntime,
  type TmuxRuntimeOptions,
} from "./src/tmux.ts";

export { CompletionDelivery, wakePiOnCompletion } from "./src/delivery.ts";

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

interface TmuxActivityState {
  sessionId: string | undefined;
  tasks: readonly string[];
}

export interface TmuxExtensionHost extends ActiveDisplayCommandHost, CompletionDeliveryHost {
  readonly events?: ActivityBus;
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

function registerLifecycleHandlers(input: {
  readonly activity: ActivityProvider | undefined;
  readonly activityState: TmuxActivityState;
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
    input.activityState.sessionId = sessionManager.getSessionId();
    input.activity?.start(input.activityState.sessionId);
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
    input.delivery.deferAfterCompaction(context);
  });
  input.host.on("agent_start", (_event: unknown, context?: CompletionDeliveryContext): void => {
    if (context !== undefined) {
      input.delivery.setContext(context);
    }
  });
  input.host.on("agent_settled", (_event: unknown, context?: CompletionDeliveryContext): void => {
    if (context !== undefined) {
      input.delivery.deferAgentSettled(context);
    }
  });
  input.host.on(
    "session_before_compact",
    (_event: unknown, context?: CompletionDeliveryContext): void =>
      input.delivery.beforeCompaction(context),
  );
  input.host.on("session_compact", (_event: unknown, context?: CompletionDeliveryContext): void => {
    input.runtime.reconcile();
    input.delivery.deferAfterCompaction(context);
  });
  input.host.on(
    "session_compact_failed",
    (_event: unknown, context?: CompletionDeliveryContext): void => {
      input.runtime.reconcile();
      input.delivery.deferAfterCompaction(context);
    },
  );
  input.host.on("session_shutdown", (): void => {
    input.activity?.stop();
    input.activityState.sessionId = undefined;
    input.activityState.tasks = [];
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
  const activityState: TmuxActivityState = { sessionId: undefined, tasks: [] };
  const activity: ActivityProvider | undefined =
    host.events === undefined
      ? undefined
      : new ActivityProvider(host.events, () => ({
          source: "tmux",
          ownsContinuation: false,
          pendingDelivery: delivery.hasPending(),
          tasks: activityState.tasks,
        }));
  const runtime: TmuxRuntime = new TmuxRuntime({
    ...runtimeOptions,
    onActiveChange: (launches: readonly TmuxLaunch[]): void => {
      activityState.tasks = launches.map((launch) => launch.sessionName);
      activeDisplay.update(launches);
    },
    onComplete: (completion: Completion): void => delivery.complete(completion),
    onOverdue: (launches: readonly TmuxLaunch[]): void => delivery.overdue(launches),
    onTrack: (launch: TmuxLaunch): void => {
      persistTmuxLaunch(host.appendEntry, launch);
      if (host.events !== undefined && activityState.sessionId !== undefined) {
        announceTask(host.events, { sessionId: activityState.sessionId, name: launch.sessionName });
      }
    },
  });
  const rewriter: AutomaticTmuxRewriter = new AutomaticTmuxRewriter(runtime);

  registerDisplayCommand(host, activeDisplay);
  registerTmuxTool(host, runtime);

  registerLifecycleHandlers({
    activity,
    activityState,
    activeDisplay,
    cleaner,
    delivery,
    host,
    recovery,
    rewriter,
    runtime,
  });
}
