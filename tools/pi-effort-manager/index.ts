// This TypeScript file is executed with Bun.
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { EffortController } from "./src/controller.ts";
import { ProgressTextController } from "./src/progress.ts";
import { STUCK_NOTE } from "./src/recovery-effort.ts";
import { roundResults, toolResultEvidence } from "./src/tool-signature.ts";

interface LifecycleControllers {
  readonly controller: EffortController;
  readonly progress: ProgressTextController;
}

const SESSION_POLICY_KEYS = new Map([
  ["start", "startEffort"],
  ["min", "startEffort"],
  ["end", "endEffort"],
  ["compact", "compactionEffort"],
  ["compaction", "compactionEffort"],
  ["reset", "compactionResetInterval"],
  ["reset-effort", "compactionResetEffort"],
] satisfies readonly (readonly [
  string,
  (
    | "startEffort"
    | "endEffort"
    | "compactionEffort"
    | "compactionResetEffort"
    | "compactionResetInterval"
  ),
])[]);

function registerFlags(pi: ExtensionAPI): void {
  pi.registerFlag("dynamic-effort", {
    description: "Dynamic effort policy (on|off)",
    type: "string",
  });
}

function registerShortcuts(pi: ExtensionAPI, controller: EffortController): void {
  pi.registerShortcut("ctrl+shift+e", {
    description: "Toggle dynamic effort",
    handler: (context): void => controller.setDynamic(context, !controller.dynamicEnabled),
  });
}

function handleAutoCommand(
  tokens: readonly string[],
  controller: EffortController,
  context: ExtensionContext,
): boolean {
  const [, action = "status", value] = tokens;
  if (action === "status") {
    context.ui.notify(controller.status(context), "info");
    return true;
  }
  if (action === "on" || action === "off") {
    controller.setDynamic(context, action === "on");
    return true;
  }
  const key = SESSION_POLICY_KEYS.get(action);
  if (key === undefined || value === undefined || tokens.length !== 3) {
    return false;
  }
  controller.setSessionPolicy(context, key, value);
  return true;
}

function registerCommands(pi: ExtensionAPI, controller: EffortController): void {
  pi.registerCommand("dynamic-effort", {
    description: "Manage dynamic reasoning effort",
    handler: async (args, context): Promise<void> => {
      if (
        handleAutoCommand(
          ["auto", ...args.trim().split(/\s+/u).filter(Boolean)],
          controller,
          context,
        )
      ) {
        return;
      }
      context.ui.notify(
        "Usage: /dynamic-effort {on|off|status|start|end|compact|reset-effort|reset <value|default>}",
        "error",
      );
    },
  });
  pi.registerCommand("fast", {
    description: "Toggle OpenAI/Codex priority service tier",
    handler: async (args, context): Promise<void> => {
      const action = args.trim();
      if (action !== "" && action !== "on" && action !== "off") {
        context.ui.notify("Usage: /fast [on|off]", "error");
        return;
      }
      controller.setFast(context, action === "" ? !controller.fastMode() : action === "on");
    },
  });
}

function registerLifecycle(pi: ExtensionAPI, controllers: LifecycleControllers): void {
  const { controller, progress } = controllers;
  pi.on("before_provider_request", (event, context) =>
    controller.providerPayload(event.payload, context),
  );
  pi.on("session_start", (_event, context): void => {
    controller.sessionStart(context, pi.getFlag("dynamic-effort"));
    progress.reset(controller.progressTextSettings());
  });
  pi.on("session_tree", (_event, context): void => {
    controller.sessionStart(context);
    progress.reset(controller.progressTextSettings());
  });
  pi.on("model_select", (_event, context): void => {
    if (controller.modelSelected(context)) {
      progress.schedule("effort-change");
    }
  });
  pi.on("before_agent_start", (_event, context): void => {
    if (controller.beforeAgentStart(context)) {
      progress.schedule("effort-change");
    }
  });
  pi.on("context_with_system", (event) => progress.context(event.messages));
  pi.on("turn_end", (event, context): void => {
    if (
      controller.turnEnded(context, {
        id: event.messageEntryId,
        results: roundResults(event.message, event.toolResults),
      })
    ) {
      progress.schedule("effort-change");
    }
  });
  pi.on("tool_result", (event, context) =>
    controller.claimStuckNote(context, toolResultEvidence(event))
      ? { content: [...event.content, { text: STUCK_NOTE, type: "text" }] }
      : undefined,
  );
  pi.on("message_end", (event, context): void => {
    if (event.message.role === "assistant" && event.message.usage.reasoning !== undefined) {
      controller.observeReasoning(context, event.message.usage.reasoning);
    }
  });
  pi.on("session_before_compact", (_event, context): void => controller.beforeCompaction(context));
  pi.on("session_compact", (_event, context): void => {
    if (controller.compacted(context)) {
      progress.schedule("compaction");
    }
  });
  pi.on("session_compact_failed", (_event, context): void => controller.compactionFailed(context));
}

export default function effortManager(pi: ExtensionAPI): void {
  const controller = new EffortController(pi);
  const progress = new ProgressTextController(controller.progressTextSettings());
  registerFlags(pi);
  registerShortcuts(pi, controller);
  registerCommands(pi, controller);
  registerLifecycle(pi, { controller, progress });
}
