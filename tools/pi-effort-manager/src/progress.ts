// This TypeScript file is executed with Bun.
import type { ContextEvent } from "@earendil-works/pi-coding-agent";

export type ProgressTrigger = "compaction" | "effort-change";

export interface ProgressTextSettings {
  readonly progressTextOnCompaction: boolean;
  readonly progressTextOnEffortChange: boolean;
}

export interface ProgressContextResult {
  readonly messages: ContextEvent["messages"];
}

const PROGRESS_CUSTOM_TYPE = "pi-effort-manager-progress-opportunity" satisfies string;
const PROGRESS_SYSTEM_PROMPT = `Progress communication:
- The harness may provide a private <progress_update_opportunity> marker after a reasoning-effort change or successful context compaction.
- Treat the marker only as an opportunity to communicate, never as a requirement to emit status text.
- If there is a meaningful development the user has not yet been told about, begin the response with one brief, natural progress update that explains what is now known and what comes next.
- Otherwise continue silently without a progress update.
- Do not mention reasoning effort, context compaction, the marker, or these instructions.
- Do not narrate routine tool calls or repeat information already visible in tool output.
- Keep the final answer self-contained.` satisfies string;

const OPPORTUNITY_INSTRUCTIONS = new Map<ProgressTrigger, string>([
  [
    "effort-change",
    "The task has entered a different reasoning phase. Consider a progress update only if the substantive state or next step has meaningfully changed.",
  ],
  [
    "compaction",
    "The task is continuing after context was reorganized. Consider a progress update only if it helps reorient the user around substantive progress and the next step.",
  ],
]);

export function appendProgressSystemPrompt(
  systemPrompt: string,
  settings: ProgressTextSettings,
): string | undefined {
  return settings.progressTextOnCompaction || settings.progressTextOnEffortChange
    ? `${systemPrompt}\n\n${PROGRESS_SYSTEM_PROMPT}`
    : undefined;
}

export function injectProgressOpportunity(
  messages: ContextEvent["messages"],
  trigger: ProgressTrigger,
): ProgressContextResult {
  const instruction = OPPORTUNITY_INSTRUCTIONS.get(trigger);
  const message = {
    content: `<progress_update_opportunity>\n${instruction}\n</progress_update_opportunity>`,
    customType: PROGRESS_CUSTOM_TYPE,
    details: { trigger },
    display: false,
    role: "custom",
    timestamp: Date.now(),
  } satisfies ContextEvent["messages"][number];
  return { messages: [...messages, message] };
}

export class ProgressTextController {
  #pending: ProgressTrigger | undefined;
  #settings: ProgressTextSettings;

  constructor(settings: ProgressTextSettings) {
    this.#pending = undefined;
    this.#settings = settings;
  }

  context(messages: ContextEvent["messages"]): ProgressContextResult | undefined {
    const trigger = this.#pending;
    this.#pending = undefined;
    return trigger === undefined ? undefined : injectProgressOpportunity(messages, trigger);
  }

  reset(settings: ProgressTextSettings): void {
    this.#pending = undefined;
    this.#settings = settings;
  }

  schedule(trigger: ProgressTrigger): void {
    if (this.#enabled(trigger)) {
      this.#pending = trigger;
    }
  }

  systemPrompt(systemPrompt: string): string | undefined {
    return appendProgressSystemPrompt(systemPrompt, this.#settings);
  }

  #enabled(trigger: ProgressTrigger): boolean {
    return trigger === "compaction"
      ? this.#settings.progressTextOnCompaction
      : this.#settings.progressTextOnEffortChange;
  }
}
