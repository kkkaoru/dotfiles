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

export function registerAgentLoop(host: StartLoopHost, runtime: LoopRuntime): void {
  host.registerTool({
    name: "start_loop",
    label: "Start Loop",
    description:
      "Autonomously start a self-paced loop for the user's established task in the current turn. Refuses existing or paused loops. Never resumes safe mode or grants new permissions.",
    executionMode: "sequential",
    parameters: startSchema,
    promptSnippet: "Start a self-paced loop for authorized ongoing work",
    promptGuidelines: [
      "Use start_loop autonomously when the user's established task needs repeated work or later checks; do not invent unrelated work.",
      "start_loop adopts the current turn without sending a duplicate prompt. Finish with loop_wakeup or loop_complete; reuse an existing goal or loop rather than duplicating pacing.",
      "Never bypass a user pause or safe-mode stop by starting another loop or goal. Only the user can resume stopped automation.",
    ],
    async execute(_id, params, _signal, _onUpdate, context) {
      runtime.startFromAgent(params.prompt, context);
      return {
        content: [{ type: "text", text: commandPrompt(params.prompt.trim()) }],
        details: { prompt: params.prompt.trim() },
      };
    },
  });
}
