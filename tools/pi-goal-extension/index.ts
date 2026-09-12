// Runs with Bun. Registers /goal and its lifecycle hooks; not a barrel module.
import type {
  ExtensionAPI,
  ExtensionContext,
} from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { GOAL_USAGE, parseGoalCommand } from "./src/parser.ts";
import {
  goalGuidance,
  goalSummary,
  runError,
  usageTokens,
} from "./src/presentation.ts";
import { GOAL_MESSAGE_PREFIX, GoalRuntime } from "./src/runtime.ts";

export interface GoalExtensionHost
  extends Pick<
    ExtensionAPI,
    | "events"
    | "on"
    | "registerCommand"
    | "registerTool"
    | "sendUserMessage"
    | "appendEntry"
  > {}
interface BridgeState {
  runtime: GoalRuntime | null;
  compacting: boolean;
  uiBusy: boolean;
}
interface AttachInput {
  readonly pi: GoalExtensionHost;
  readonly state: BridgeState;
  readonly context: ExtensionContext;
}
interface CommandInput {
  readonly args: string;
  readonly context: ExtensionContext;
  readonly state: BridgeState;
}

function requireRuntime(state: BridgeState): GoalRuntime {
  if (state.runtime === null)
    throw new Error("Goal session is not initialized.");
  return state.runtime;
}

function attach(input: AttachInput): void {
  input.state.runtime?.shutdown();
  input.state.compacting = false;
  input.state.uiBusy = false;
  input.state.runtime = new GoalRuntime({
    bus: input.pi.events,
    sessionId: input.context.sessionManager.getSessionId(),
    isReady: () =>
      !input.state.compacting &&
      !input.state.uiBusy &&
      input.context.isIdle() &&
      !input.context.hasPendingMessages(),
    send: (text) => input.pi.sendUserMessage(text, { deliverAs: "followUp" }),
    persist: (type, data) => input.pi.appendEntry(type, data),
    display: (goal) =>
      input.context.ui.setStatus(
        "goal",
        goal === null ? undefined : `goal: ${goal.status}`,
      ),
  });
  input.state.runtime.restore(input.context.sessionManager.getBranch());
}

async function command(input: CommandInput): Promise<void> {
  const parsed = parseGoalCommand(input.args);
  const runtime: GoalRuntime = requireRuntime(input.state);
  switch (parsed.kind) {
    case "create":
      runtime.start(parsed);
      break;
    case "status":
      break;
    case "pause":
      runtime.pause();
      break;
    case "resume":
      runtime.resume();
      break;
    case "clear":
      runtime.clear();
      break;
    case "budget":
      runtime.budget(parsed.tokens);
      break;
    case "edit": {
      if (parsed.objective !== null) {
        runtime.edit(parsed.objective);
        break;
      }
      runtime.pause();
      const original = runtime.state;
      const text: string | undefined = await input.context.ui.editor(
        "Edit goal (remains paused)",
        original?.objective,
      );
      if (text === undefined) return;
      if (input.state.runtime !== runtime || runtime.state !== original)
        throw new Error(
          "Goal changed while the editor was open; retry editing.",
        );
      runtime.edit(text);
      break;
    }
  }
  input.context.ui.notify(
    `${goalSummary(runtime.state)}\nPause/clear do not stop independent loops or tmux processes.`,
    "info",
  );
}

function registerTools(pi: GoalExtensionHost, state: BridgeState): void {
  pi.registerTool({
    name: "get_goal",
    label: "Get Goal",
    description:
      "Read the explicitly configured session goal and its usage. Does not create a goal.",
    executionMode: "parallel",
    parameters: Type.Object({}),
    async execute() {
      const goal = requireRuntime(state).state;
      return {
        content: [{ type: "text", text: JSON.stringify(goal) }],
        details: { goal },
      };
    },
  });
  pi.registerTool({
    name: "update_goal",
    label: "Audit Goal",
    description:
      "Audit an active goal as verified complete or report the same genuine blocker once per turn. Three consecutive blocked turns stop continuation. Cannot resume or replace goals.",
    executionMode: "sequential",
    parameters: Type.Object({
      status: Type.Union([Type.Literal("complete"), Type.Literal("blocked")]),
      reason: Type.String({
        minLength: 1,
        description:
          "Completion evidence, or a stable unchanged blocker reason.",
      }),
    }),
    async execute(_id, params) {
      const runtime: GoalRuntime = requireRuntime(state);
      runtime.update(params.status, params.reason);
      return {
        content: [{ type: "text", text: goalSummary(runtime.state) }],
        details: { goal: runtime.state },
      };
    },
  });
  pi.registerTool({
    name: "goal_wait",
    label: "Wait for Goal",
    description:
      "Schedule a justified later goal check (60–3,600 seconds). Prefer live tmux notifications or existing loop pacing. Never use while immediately actionable work remains.",
    executionMode: "sequential",
    parameters: Type.Object({
      delaySeconds: Type.Integer({ minimum: 60, maximum: 3600 }),
      reason: Type.String({ minLength: 1 }),
    }),
    async execute(_id, params) {
      const runtime: GoalRuntime = requireRuntime(state);
      runtime.wait(params);
      return {
        content: [
          {
            type: "text",
            text: "Goal check scheduled; no duplicate loop wakeup is needed.",
          },
        ],
        details: { goal: runtime.state },
      };
    },
  });
}

export default function goalExtension(pi: GoalExtensionHost): void {
  const state: BridgeState = {
    runtime: null,
    compacting: false,
    uiBusy: false,
  };
  registerTools(pi, state);
  pi.registerCommand("goal", {
    description: GOAL_USAGE,
    handler: async (args, context) => {
      try {
        await command({ args, context, state });
      } catch (error: unknown) {
        context.ui.notify(
          error instanceof Error ? error.message : "Goal command failed.",
          "error",
        );
      }
    },
  });
  pi.on("session_start", (_event, context) => attach({ pi, state, context }));
  pi.on("session_tree", (_event, context) => attach({ pi, state, context }));
  pi.on("session_shutdown", () => {
    state.runtime?.shutdown();
    state.runtime = null;
  });
  pi.on("input", (event) => {
    if (event.source !== "extension") {
      state.runtime?.invalidateTicket();
      return;
    }
    if (!event.text.startsWith(GOAL_MESSAGE_PREFIX)) return;
    return state.runtime?.accept(event.text) === true
      ? { action: "continue" }
      : { action: "handled" };
  });
  pi.on("before_agent_start", (event) => {
    const guidance: string = goalGuidance(state.runtime?.state ?? null);
    return guidance.length === 0
      ? undefined
      : { systemPrompt: `${event.systemPrompt}\n\n${guidance}` };
  });
  pi.on("agent_start", () => state.runtime?.begin());
  pi.on("message_end", (event) =>
    state.runtime?.recordTokens(usageTokens(event.message)),
  );
  pi.on("tool_execution_end", (event) =>
    state.runtime?.recordTool(event.toolName),
  );
  pi.on("agent_end", (event) => state.runtime?.end(runError(event.messages)));
  pi.on("agent_settled", () => state.runtime?.settled());
  pi.on("session_before_compact", () => {
    state.compacting = true;
  });
  pi.on("session_compact", (event) => {
    state.compacting = false;
    state.runtime?.recordTokens(usageTokens(event.compactionEntry));
  });
  pi.on("session_compact_failed", () => {
    state.compacting = false;
  });
  pi.on("ui_prompt_start", () => {
    state.uiBusy = true;
  });
  pi.on("ui_prompt_end", () => {
    state.uiBusy = false;
  });
}
