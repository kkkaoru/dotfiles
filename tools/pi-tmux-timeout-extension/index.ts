// This TypeScript file is executed with Bun.
import { ActivityProvider, announceTask, type ActivityBus } from "./src/goal-activity.ts";
import type { Static } from "typebox";
import { registerTmuxTool } from "./src/register-tool.ts";
import { durableTmuxHost } from "./src/durable-host.ts";
import { eventToolCallId, toolResultFailed } from "./src/tool-event.ts";
import type { tmuxExecSchema } from "./src/tool-schema.ts";
import { ArtifactCleaner, type ArtifactCleanerOptions } from "./src/cleanup.ts";
import { ActiveTaskDisplay } from "./src/active-display.ts";
import { inspectOverdue, remindOverdue, restoreOverdue } from "./src/overdue-display.ts";
import {
  CompletionDelivery,
  type CompletionDeliveryContext,
  type CompletionDeliveryHost,
} from "./src/delivery.ts";
import { type ActiveDisplayCommandHost, registerDisplayCommand } from "./src/display-command.ts";
import {
  markCompletionDelivered,
  TMUX_DELIVERED_ENTRY_TYPE,
  persistTmuxLaunch,
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
  readonly killed?: boolean;
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
  | "context"
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
  readonly flush?: () => Promise<void>;
  readonly prepareLaunch?: (launch: TmuxLaunch) => Promise<void>;
  readonly monitor?: (reconcile: () => void) => () => void;
  readonly pendingTasks?: () => readonly string[];
  readonly complete?: (completion: Completion, deliver: () => void) => void;
  readonly exec: (
    command: string,
    args: readonly string[],
    options?: ExecOptions,
  ) => Promise<ExecResult>;
  readonly on: (
    event: TmuxLifecycleEvent,
    handler: (event: unknown, context?: CompletionDeliveryContext) => unknown,
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

class AutomaticTmuxRewriter {
  readonly #pending = new Map<string, TmuxLaunch>();
  readonly #runtime: TmuxRuntime;

  constructor(runtime: TmuxRuntime) {
    this.#runtime = runtime;
  }

  async toolCall(event: unknown, host: TmuxExtensionHost): Promise<void> {
    const toolCallId: string | undefined = eventToolCallId(event);
    const extracted: ExtractedBashInput | undefined = normalizedBashInput(toolCallInput(event));
    if (toolCallId === undefined || extracted === undefined) {
      return;
    }
    const launch: TmuxLaunch | undefined = this.#runtime.rewriteLongBash(extracted.input);
    if (launch === undefined) {
      return;
    }
    await host.prepareLaunch?.(launch);
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
  input.host.on("context", (event: unknown) => {
    input.runtime.reconcile();
    return input.delivery.injectOverdue(event);
  });
  input.host.on("tool_call", async (event: unknown): Promise<void> =>
    input.rewriter.toolCall(event, input.host),
  );
  input.host.on("tool_result", (event: unknown): void => {
    input.rewriter.toolResult(event);
    inspectOverdue(event, {
      display: input.activeDisplay,
      delivery: input.delivery,
      runtime: input.runtime,
      persist: input.host.appendEntry,
    });
  });
  input.host.on("session_start", (_event: unknown, context?: CompletionDeliveryContext): void => {
    const sessionManager = context?.sessionManager;
    if (context === undefined || sessionManager === undefined) {
      return;
    }
    input.cleaner.start();
    input.activityState.sessionId = sessionManager.getSessionId();
    input.activity?.start(input.activityState.sessionId);
    restoreOverdue({
      context,
      sessionManager,
      display: input.activeDisplay,
      delivery: input.delivery,
      runtime: input.runtime,
      recovery: input.recovery,
    });
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
  const afterCompaction = (_event: unknown, context?: CompletionDeliveryContext): void => {
    input.runtime.reconcile();
    input.delivery.deferAfterCompaction(context);
  };
  input.host.on("session_compact", afterCompaction);
  input.host.on("session_compact_failed", afterCompaction);
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

export function registerTmux(
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
          onDelivered: (completion: Completion): void => {
            host.appendEntry?.(TMUX_DELIVERED_ENTRY_TYPE, {
              sessionName: completion.launch.sessionName,
            });
            markCompletionDelivered(completion.launch, recovery?.operations);
          },
        },
  );
  const activityState: TmuxActivityState = { sessionId: undefined, tasks: [] };
  const activity: ActivityProvider | undefined =
    host.events === undefined
      ? undefined
      : new ActivityProvider(host.events, () => ({
          source: "tmux",
          ownsContinuation: false,
          pendingDelivery: delivery.hasPending() || (host.pendingTasks?.().length ?? 0) > 0,
          pendingTasks: [...delivery.pendingTaskNames(), ...(host.pendingTasks?.() ?? [])],
          tasks: activityState.tasks,
        }));
  const runtime: TmuxRuntime = new TmuxRuntime({
    ...runtimeOptions,
    ...(host.monitor === undefined ? {} : { monitor: host.monitor }),
    onActiveChange: (launches: readonly TmuxLaunch[]): void => {
      activityState.tasks = launches.map((launch) => launch.sessionName);
      activeDisplay.update(launches);
    },
    onComplete: (completion: Completion): void => {
      if (host.complete === undefined) {
        delivery.complete(completion);
      } else {
        host.complete(completion, () => delivery.complete(completion));
      }
    },
    onOverdue: (launches: readonly TmuxLaunch[]): void =>
      remindOverdue(launches, {
        display: activeDisplay,
        delivery,
        runtime,
        persist: host.appendEntry,
      }),
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

export default function tmuxTimeoutExtension(host: TmuxExtensionHost): void {
  registerTmux(durableTmuxHost(host));
}
