// This TypeScript file is executed with Bun.
import { ACTIVE_DISPLAY_ENTRY_TYPE, type ActiveTaskDisplay } from "./active-display.ts";
import type { CompletionDeliveryContext } from "./delivery.ts";

export interface TmuxCommandDefinition {
  readonly description: string;
  readonly handler: (args: string, context: CompletionDeliveryContext) => void | Promise<void>;
}

export interface ActiveDisplayCommandHost {
  readonly appendEntry?: (customType: string, data: unknown) => void;
  readonly registerCommand?: (name: string, definition: TmuxCommandDefinition) => void;
}

interface DisplayCommandInput {
  readonly args: string;
  readonly context: CompletionDeliveryContext;
  readonly display: ActiveTaskDisplay;
  readonly host: ActiveDisplayCommandHost;
}

function persistActiveDisplayState(
  host: ActiveDisplayCommandHost,
  display: ActiveTaskDisplay,
): void {
  host.appendEntry?.(ACTIVE_DISPLAY_ENTRY_TYPE, display.state());
}

function handleTmuxTasksCommand(input: DisplayCommandInput): void {
  const action: string = input.args.trim() || "status";
  if (action === "status") {
    input.context.ui.notify(
      `tmux tasks: active=${String(input.display.activeCount())} visible=${String(input.display.visibleCount())}`,
      "info",
    );
    return;
  }
  if (action === "clear") {
    const dismissed: number = input.display.dismissActive();
    persistActiveDisplayState(input.host, input.display);
    input.context.ui.notify(`Cleared ${String(dismissed)} tmux task display(s).`, "info");
    return;
  }
  if (action === "hide" || action === "show") {
    input.display.setHidden(action === "hide");
    persistActiveDisplayState(input.host, input.display);
    input.context.ui.notify(`Tmux task display ${action === "hide" ? "hidden" : "shown"}.`, "info");
    return;
  }
  if (action === "reset") {
    input.display.reset();
    persistActiveDisplayState(input.host, input.display);
    input.context.ui.notify("Tmux task display reset.", "info");
    return;
  }
  input.context.ui.notify("Usage: /tmux-tasks [status|clear|hide|show|reset]", "error");
}

export function registerDisplayCommand(
  host: ActiveDisplayCommandHost,
  display: ActiveTaskDisplay,
): void {
  host.registerCommand?.("tmux-tasks", {
    description: "Control active tmux task display for this session",
    handler: (args, context): void => handleTmuxTasksCommand({ args, context, display, host }),
  });
}
