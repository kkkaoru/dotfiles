// This TypeScript file is executed with Bun.
import type { TmuxExtensionHost } from "../index.ts";
import { ACTIVE_DISPLAY_ENTRY_TYPE, type ActiveTaskDisplay } from "./active-display.ts";
import type { CompletionDelivery } from "./delivery.ts";
import { formatLocalTimestamp } from "./policy.ts";
import type { TmuxLaunch, TmuxRuntime } from "./tmux.ts";
import { tmuxCheckInSchema } from "./tool-schema.ts";

export interface CheckInDependencies {
  readonly delivery: Pick<CompletionDelivery, "resolved">;
  readonly display: Pick<ActiveTaskDisplay, "clearAcknowledged" | "state">;
  readonly persist: ((type: string, data: unknown) => void) | undefined;
  readonly runtime: Pick<TmuxRuntime, "reschedule">;
}

export interface CheckInResult {
  readonly launch: TmuxLaunch;
  readonly text: string;
}

function checkInText(launch: TmuxLaunch, nextCheckSeconds: number, now: number): string {
  const nextCheck = new Date(now + nextCheckSeconds * 1000);
  return [
    `tmux check-in recorded for ${launch.sessionName}.`,
    `Next check-in: ${formatLocalTimestamp(nextCheck, "submitted")} (${String(Math.ceil(nextCheckSeconds / 60))} min).`,
    "The job is still running and tracked; this does not stop it or prove completion.",
  ].join("\n");
}

/** Resolve one overdue notice for a bounded next check without touching the tmux process. */
export function recordCheckIn(
  dependencies: CheckInDependencies,
  sessionName: string,
  nextCheckSeconds: number,
): CheckInResult {
  const now: number = Date.now();
  const launch: TmuxLaunch | undefined = dependencies.runtime.reschedule(
    sessionName,
    nextCheckSeconds,
    now,
  );
  if (launch === undefined) {
    throw new Error(
      `No active tmux task named ${sessionName}; it may have finished or been cleared.`,
    );
  }
  dependencies.delivery.resolved([launch]);
  if (dependencies.display.clearAcknowledged([launch])) {
    dependencies.persist?.(ACTIVE_DISPLAY_ENTRY_TYPE, dependencies.display.state());
  }
  return { launch, text: checkInText(launch, nextCheckSeconds, now) };
}

export function registerCheckInTool(
  host: TmuxExtensionHost,
  dependencies: CheckInDependencies,
): void {
  host.registerTool({
    description:
      "Record a bounded next check-in for a live tmux task that exceeded its estimate. Call it after inspecting the job when the work should continue; it reschedules the estimate and clears the current overdue notice without stopping or completing the job.",
    executionMode: "parallel",
    label: "Tmux Check-In",
    name: "tmux_check_in",
    parameters: tmuxCheckInSchema,
    promptGuidelines: [
      "On a tmux overdue check-in, inspect the exact job first, then either stop that specific tmux session or call tmux_check_in with its sessionName and a realistic nextCheckSeconds. Use this to keep a long-running watcher without repeating the same notice.",
      "Never use tmux_check_in to hide an uninspected job, and never report the job as complete from a check-in. It neither stops the process nor proves completion.",
    ],
    promptSnippet: "Acknowledge an overdue tmux check-in and schedule its next check",
    async execute(_toolCallId, params) {
      const result: CheckInResult = recordCheckIn(
        dependencies,
        params.sessionName,
        params.nextCheckSeconds,
      );
      await host.flush?.();
      return { content: [{ text: result.text, type: "text" }], details: result.launch };
    },
  });
}
