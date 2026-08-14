import { type Static, Type, type TSchema } from "typebox";
import { LoopRuntime, type LoopContext, type LoopHost } from "./src/runtime.ts";

const wakeupSchema = Type.Object({
  delaySeconds: Type.Integer({
    description: "Delay before the next useful check, from 60 to 3,600 seconds",
    maximum: 3600,
    minimum: 60,
  }),
  prompt: Type.String({ description: "Prompt for the next loop tick", minLength: 1 }),
  reason: Type.String({ description: "Short reason for choosing this delay", minLength: 1 }),
}) satisfies TSchema;

type LifecycleEvent = "session_shutdown" | "session_start";

interface ToolResult {
  readonly content: readonly [{ readonly text: string; readonly type: "text" }];
  readonly details: { readonly id: number; readonly scheduledInSeconds: number };
}

export interface LoopToolDefinition {
  readonly description: string;
  readonly executionMode: "sequential";
  readonly execute: (
    toolCallId: string,
    params: Static<typeof wakeupSchema>,
    signal: AbortSignal | undefined,
    onUpdate: unknown,
    context: LoopContext,
  ) => Promise<ToolResult>;
  readonly label: string;
  readonly name: "loop_wakeup";
  readonly parameters: typeof wakeupSchema;
  readonly promptGuidelines: readonly string[];
  readonly promptSnippet: string;
}

export interface LoopCommandDefinition {
  readonly description: string;
  readonly getArgumentCompletions: (
    prefix: string,
  ) => readonly { readonly label: string; readonly value: string }[] | null;
  readonly handler: (args: string, context: LoopContext) => void;
}

export interface LoopExtensionHost extends LoopHost {
  readonly on: (
    event: LifecycleEvent,
    handler: (event: unknown, context: LoopContext) => void,
  ) => void;
  readonly registerCommand: (name: "loop", definition: LoopCommandDefinition) => void;
  readonly registerTool: (definition: LoopToolDefinition) => void;
}

const COMPLETIONS = ["list", "clear", "5m ", "30m ", "1h "] satisfies string[];

export default function loopExtension(host: LoopExtensionHost): void {
  const runtime: LoopRuntime = new LoopRuntime(host);

  host.registerTool({
    description:
      "Schedule one self-paced loop wakeup. Use only when another useful check remains; do not use when work is complete or blocked. Delays are limited to 60-3,600 seconds.",
    executionMode: "sequential",
    label: "Loop Wakeup",
    name: "loop_wakeup",
    parameters: wakeupSchema,
    promptGuidelines: [
      "Use loop_wakeup only for a useful continuation of an active /loop; never keep a completed or user-blocked loop alive.",
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
    handler: (args: string, context: LoopContext): void => runtime.command(args, context),
  });

  host.on("session_start", (_event: unknown, context: LoopContext): void =>
    runtime.setContext(context),
  );
  host.on("session_shutdown", (): void => {
    runtime.clear();
  });
}
