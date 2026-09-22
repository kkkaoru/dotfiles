// This TypeScript file is executed with Bun.
import { type Static, Type, type TSchema } from "typebox";
import { registerAgentLoop, type StartLoopToolDefinition } from "./src/agent-start.ts";
import {
  failedAgentRun,
  pauseAfterAgentFailure,
  pauseAfterCompactionFailure,
} from "./src/compaction-failure.ts";
import { ActivityProvider, type ActivityBus } from "./src/goal-activity.ts";
import { parseLoopCommand } from "./src/parser.ts";
import { LoopRuntime, type LoopContext, type LoopHost } from "./src/runtime.ts";
import { createLoopState, latestLoopState, type LoopRuntimeState } from "./src/state.ts";
import { WatchedLoopTasks } from "./src/watched-tasks.ts";

const wakeupSchema = Type.Object({
  delaySeconds: Type.Integer({
    description: "Delay before the next useful check, from 60 to 3,600 seconds",
    maximum: 3600,
    minimum: 60,
  }),
  prompt: Type.String({ description: "Prompt for the next loop tick", minLength: 1 }),
  reason: Type.String({ description: "Short reason for choosing this delay", minLength: 1 }),
}) satisfies TSchema;
const completeSchema = Type.Object({
  reason: Type.String({
    description: "Short completion result or blocker that makes another loop tick unnecessary",
    minLength: 1,
  }),
}) satisfies TSchema;

type LifecycleEvent =
  | "agent_end"
  | "agent_settled"
  | "session_compact"
  | "session_compact_failed"
  | "session_shutdown"
  | "session_start";

interface WakeupToolResult {
  readonly content: readonly [{ readonly text: string; readonly type: "text" }];
  readonly details: { readonly id: number; readonly scheduledInSeconds: number };
}
interface CompleteToolResult {
  readonly content: readonly [{ readonly text: string; readonly type: "text" }];
  readonly details: { readonly reason: string };
}

export interface LoopWakeupToolDefinition {
  readonly description: string;
  readonly executionMode: "parallel";
  readonly execute: (
    toolCallId: string,
    params: Static<typeof wakeupSchema>,
    signal: AbortSignal | undefined,
    onUpdate: unknown,
    context: LoopContext,
  ) => Promise<WakeupToolResult>;
  readonly label: string;
  readonly name: "loop_wakeup";
  readonly parameters: typeof wakeupSchema;
  readonly promptGuidelines: readonly string[];
  readonly promptSnippet: string;
}
export interface LoopCompleteToolDefinition {
  readonly description: string;
  readonly executionMode: "parallel";
  readonly execute: (
    toolCallId: string,
    params: Static<typeof completeSchema>,
    signal: AbortSignal | undefined,
    onUpdate: unknown,
    context: LoopContext,
  ) => Promise<CompleteToolResult>;
  readonly label: string;
  readonly name: "loop_complete";
  readonly parameters: typeof completeSchema;
  readonly promptGuidelines: readonly string[];
  readonly promptSnippet: string;
}
export type LoopToolDefinition =
  | LoopCompleteToolDefinition
  | LoopWakeupToolDefinition
  | StartLoopToolDefinition;

export interface LoopCommandDefinition {
  readonly description: string;
  readonly getArgumentCompletions: (
    prefix: string,
  ) => readonly { readonly label: string; readonly value: string }[] | null;
  readonly handler: (args: string, context: LoopContext) => void;
}

export interface LoopExtensionHost extends LoopHost {
  readonly events?: ActivityBus;
  readonly on: (
    event: LifecycleEvent,
    handler: (event: unknown, context: LoopContext) => void,
  ) => void;
  readonly registerCommand: (name: "loop", definition: LoopCommandDefinition) => void;
  readonly registerTool: (definition: LoopToolDefinition) => void;
}

const COMPLETIONS = ["list", "clear", "pause", "resume", "5m ", "30m ", "1h "] satisfies string[];

function willRetryAfterCompaction(event: unknown): boolean {
  return (
    typeof event === "object" && event !== null && "willRetry" in event && event.willRetry === true
  );
}

function registerLifecycleHandlers(
  host: LoopExtensionHost,
  runtime: LoopRuntime,
  watched: WatchedLoopTasks,
): void {
  const health = { failed: false };
  const activity: ActivityProvider | undefined =
    host.events === undefined
      ? undefined
      : new ActivityProvider(host.events, () => ({
          source: "loop",
          ownsContinuation: runtime.ownsContinuation(),
          pendingDelivery: false,
          tasks: [],
        }));
  host.on("session_start", (_event: unknown, context: LoopContext): void => {
    const restored: LoopRuntimeState =
      latestLoopState(context.sessionManager?.getEntries() ?? []) ??
      createLoopState({
        jobs: [],
        nextId: 1,
        paused: false,
        pendingContinuations: [],
        runningContinuation: undefined,
      });
    runtime.restore(restored, context);
    activity?.stop();
    const sessionId: string | undefined = context.sessionManager?.getSessionId?.();
    if (sessionId !== undefined) {
      activity?.start(sessionId);
    }
    watched.attach(host.events, sessionId, () => runtime.ownsContinuation());
  });
  host.on("session_compact", (event: unknown, context: LoopContext): void =>
    runtime.deferLifecycleContinuation((): void =>
      runtime.continueAfterCompaction(willRetryAfterCompaction(event), context),
    ),
  );
  host.on("session_compact_failed", (_event: unknown, context: LoopContext): void =>
    pauseAfterCompactionFailure(runtime, context),
  );
  host.on("agent_end", (event: unknown): void => {
    health.failed = failedAgentRun(event);
  });
  host.on("agent_settled", (_event: unknown, context: LoopContext): void => {
    if (health.failed) {
      pauseAfterAgentFailure(runtime, context);
    }
    runtime.deferLifecycleContinuation((): void =>
      runtime.agentSettled(context, watched.unfinished(host.events)),
    );
  });
  host.on("session_shutdown", (): void => {
    activity?.stop();
    runtime.shutdown();
  });
}

export default function loopExtension(host: LoopExtensionHost): void {
  const runtime: LoopRuntime = new LoopRuntime(host);
  const watched = new WatchedLoopTasks();

  host.registerTool({
    description:
      "Schedule one self-paced loop wakeup. Use when useful work remains but the next check should happen later. Delays are limited to 60-3,600 seconds.",
    executionMode: "parallel",
    label: "Loop Wakeup",
    name: "loop_wakeup",
    parameters: wakeupSchema,
    promptGuidelines: [
      "For an active self-paced /loop, do not end while immediately actionable work remains; continue working in the current turn.",
      "Before ending an active self-paced /loop tick, call exactly one terminal loop tool: loop_wakeup when a useful later check remains, or loop_complete only when the task is complete or blocked on user input.",
      "Never merely report remaining work without a terminal loop tool; a tick without loop_wakeup or loop_complete continues once, then the loop stops.",
      "When using loop_wakeup, choose the next useful delay and preserve required state in its prompt.",
    ],
    promptSnippet: "Schedule the next useful self-paced /loop tick",
    async execute(_toolCallId, params, _signal, _onUpdate, context) {
      const result = runtime.wakeup(params, context);
      return {
        content: [
          {
            text: `Scheduled loop #${String(result.id)} in ${String(result.scheduledInSeconds)} seconds.`,
            type: "text",
          },
        ],
        details: result,
      };
    },
  });

  host.registerTool({
    description:
      "Finish the active self-paced loop. Use only when its task is complete or blocked on user input; unfinished work must continue or use loop_wakeup.",
    executionMode: "parallel",
    label: "Complete Loop",
    name: "loop_complete",
    parameters: completeSchema,
    promptGuidelines: [
      "Call loop_complete only for an active self-paced /loop whose task is complete or blocked on user input.",
      "Do not call loop_complete when implementation, verification, or another authorized step remains actionable.",
    ],
    promptSnippet: "Mark the active self-paced /loop complete or user-blocked",
    async execute(_toolCallId, params, _signal, _onUpdate, context) {
      const result = runtime.complete(params.reason, context);
      watched.clear();
      return {
        content: [{ text: `Completed loop: ${result.reason}`, type: "text" }],
        details: result,
      };
    },
  });

  host.registerCommand("loop", {
    description: "Run a prompt now and continue on a self-paced or fixed schedule",
    getArgumentCompletions: (prefix: string) => {
      const matches: readonly string[] = COMPLETIONS.filter((value: string): boolean =>
        value.startsWith(prefix),
      );
      return matches.length === 0
        ? null
        : matches.map((value: string) => ({ label: value, value }));
    },
    handler: (args: string, context: LoopContext): void => {
      runtime.command(args, context);
      if (parseLoopCommand(args).kind === "clear") {
        watched.clear();
      }
    },
  });

  registerAgentLoop(host, runtime, () => watched.clear());
  registerLifecycleHandlers(host, runtime, watched);
}
