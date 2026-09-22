// This TypeScript file is executed with Bun.
import { Type, type Static } from "typebox";
import { commandPrompt } from "./helpers.ts";
import type { LoopContext, LoopRuntime } from "./runtime.ts";

const startSchema = Type.Object({
  prompt: Type.String({
    minLength: 1,
    description: "Task grounded in the current user's request, with completion criteria",
  }),
});

export interface StartLoopToolDefinition {
  readonly name: "start_loop";
  readonly label: string;
  readonly description: string;
  readonly executionMode: "sequential";
  readonly parameters: typeof startSchema;
  readonly promptSnippet: string;
  readonly promptGuidelines: readonly string[];
  readonly execute: (
    id: string,
    params: Static<typeof startSchema>,
    signal: AbortSignal | undefined,
    onUpdate: unknown,
    context: LoopContext,
  ) => Promise<{ content: readonly [{ type: "text"; text: string }]; details: { prompt: string } }>;
}
interface StartLoopHost {
  readonly registerTool: (definition: StartLoopToolDefinition) => void;
}

export function registerAgentLoop(
  host: StartLoopHost,
  runtime: LoopRuntime,
  afterStart?: () => void,
): void {
  host.registerTool({
    name: "start_loop",
    label: "Start Loop",
    description:
      "Autonomously start a self-paced loop for the user's established task in the current turn. Supersedes existing loop work, including the leftover jobs of a paused loop, so a pause never blocks new authorized work. Refuses empty tasks. Never resumes safe mode or grants new permissions.",
    executionMode: "sequential",
    parameters: startSchema,
    promptSnippet: "Start a self-paced loop for authorized ongoing work",
    promptGuidelines: [
      "Use start_loop autonomously when the user's established task needs repeated work or later checks; do not invent unrelated work.",
      "start_loop adopts the current turn without sending a duplicate prompt and replaces any existing loop jobs or retained ticks, including a paused loop, and the discarded job count is reported. Finish with loop_wakeup or loop_complete; prefer reusing an existing goal or loop over duplicating pacing.",
      "Never bypass safe mode or a goal the user paused by starting another loop or goal; only the user resumes those. A paused loop does not block new work because start_loop supersedes it.",
    ],
    async execute(_id, params, _signal, _onUpdate, context) {
      const discarded: number = runtime.startFromAgent(params.prompt, context);
      afterStart?.();
      if (discarded > 0) {
        context.ui.notify(
          `Superseded a paused loop (${String(discarded)} job(s) discarded).`,
          "warning",
        );
      }
      return {
        content: [{ type: "text", text: commandPrompt(params.prompt.trim()) }],
        details: { prompt: params.prompt.trim() },
      };
    },
  });
}
