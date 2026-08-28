// This TypeScript file is executed with Bun.
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { EffortController } from "./src/controller.ts";
import { ProgressTextController } from "./src/progress.ts";

interface LifecycleControllers {
  readonly controller: EffortController;
  readonly progress: ProgressTextController;
}

const SESSION_POLICY_KEYS = new Map([
  ["start", "startEffort"],
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
    handler: (ctx): void => controller.setDynamic(ctx, !controller.dynamicEnabled),
  });
}

function handleAutoCommand(
  tokens: readonly string[],
  controller: EffortController,
  ctx: ExtensionContext,
): boolean {
  const [, action = "status", value] = tokens;
  if (action === "status") {
    ctx.ui.notify(controller.status(ctx), "info");
    return true;
  }
  if (action === "on" || action === "off") {
    controller.setDynamic(ctx, action === "on");
    return true;
  }
  const key = SESSION_POLICY_KEYS.get(action);
  if (key === undefined || value === undefined || tokens.length !== 3) {
    return false;
  }
  controller.setSessionPolicy(ctx, key, value);
  return true;
}

function registerCommands(pi: ExtensionAPI, controller: EffortController): void {
  pi.registerCommand("dynamic-effort", {
    description: "Manage dynamic reasoning effort",
    handler: async (args, ctx): Promise<void> => {
      const tokens = ["auto", ...args.trim().split(/\s+/u).filter(Boolean)];
      if (handleAutoCommand(tokens, controller, ctx)) {
        return;
      }
      ctx.ui.notify(
        "Usage: /dynamic-effort {on|off|status|start|end|compact|reset-effort|reset <value|default>}",
        "error",
      );
    },
  });
  pi.registerCommand("fast", {
    description: "Toggle OpenAI/Codex priority service tier",
    handler: async (args, ctx): Promise<void> => {
      const action = args.trim();
      if (action !== "" && action !== "on" && action !== "off") {
        ctx.ui.notify("Usage: /fast [on|off]", "error");
        return;
      }
      controller.setFast(ctx, action === "" ? !controller.fastMode() : action === "on");
    },
  });
}

function registerLifecycle(pi: ExtensionAPI, controllers: LifecycleControllers): void {
  const { controller, progress } = controllers;
  pi.on("before_provider_request", (event, ctx) => controller.providerPayload(event.payload, ctx));
  pi.on("session_start", (_event, ctx): void => {
    controller.sessionStart(ctx, pi.getFlag("dynamic-effort"));
    progress.reset(controller.progressTextSettings());
  });
  pi.on("model_select", (_event, ctx): void => {
    if (controller.modelSelected(ctx)) {
      progress.schedule("effort-change");
    }
  });
  pi.on("before_agent_start", (event, ctx) => {
    if (controller.beforeAgentStart(ctx)) {
      progress.schedule("effort-change");
    }
    const systemPrompt = progress.systemPrompt(event.systemPrompt);
    return systemPrompt === undefined ? undefined : { systemPrompt };
  });
  pi.on("context", (event) => progress.context(event.messages));
  pi.on("turn_end", (_event, ctx): void => {
    if (controller.turnEnded(ctx)) {
      progress.schedule("effort-change");
    }
  });
  pi.on("message_end", (event, ctx): void => {
    if (event.message.role === "assistant" && event.message.usage.reasoning !== undefined) {
      controller.observeReasoning(ctx, event.message.usage.reasoning);
    }
  });
  pi.on("session_before_compact", (_event, ctx): void => controller.beforeCompaction(ctx));
  pi.on("session_compact", (_event, ctx): void => {
    controller.compacted(ctx);
    progress.schedule("compaction");
  });
  pi.on("session_compact_failed", (_event, ctx): void => controller.compactionFailed(ctx));
}

export default function effortManager(pi: ExtensionAPI): void {
  const controller = new EffortController(pi);
  const progress = new ProgressTextController(controller.progressTextSettings());
  registerFlags(pi);
  registerShortcuts(pi, controller);
  registerCommands(pi, controller);
  registerLifecycle(pi, { controller, progress });
}
