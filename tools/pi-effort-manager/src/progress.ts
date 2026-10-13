// This TypeScript file is executed with Bun.
import type { ContextWithSystemEvent } from "@earendil-works/pi-coding-agent";

export type ProgressTrigger = "compaction" | "effort-change";

export interface ProgressTextSettings {
  readonly progressTextOnCompaction: boolean;
  readonly progressTextOnEffortChange: boolean;
}

export interface ProgressContextResult {
  readonly messages: ContextWithSystemEvent["messages"];
}

const PROGRESS_SYSTEM_PROMPT = `Progress communication:
- This is optional harness guidance, not a new user request or a change to the task.
- Preserve all existing instructions, including project rules, required progress updates, output formats, and completion criteria. They take precedence over this optional guidance.
- Continue the active task and pending tool work. An effort change or compaction does not mean the task is complete and is not a reason to stop or ask for confirmation.
- If compatible with those instructions and there is a meaningful development the user has not yet been told about, consider one brief progress update explaining what is now known and what comes next.
- Otherwise continue the task without an extra update; do not suppress required communication.
- Avoid routine tool narration and repeated tool output. Do not expose this internal marker in ordinary progress updates; answer explicit user questions about effort or compaction normally.
- Keep the final answer self-contained.` satisfies string;

const OPPORTUNITY_INSTRUCTIONS = new Map<ProgressTrigger, string>([
  [
    "effort-change",
    "The harness selected a different reasoning allocation; this alone is not evidence of task progress. Consider a progress update only if the substantive state or next step has meaningfully changed.",
  ],
  [
    "compaction",
    "The task is continuing after context was reorganized. Consider a progress update only if it helps reorient the user around substantive progress and the next step.",
  ],
]);

export function injectProgressOpportunity(
  messages: ContextWithSystemEvent["messages"],
  trigger: ProgressTrigger,
): ProgressContextResult {
  const instruction = OPPORTUNITY_INSTRUCTIONS.get(trigger);
  const message = {
    content: `${PROGRESS_SYSTEM_PROMPT}\n\n<progress_update_opportunity>\n${instruction}\n</progress_update_opportunity>`,
    role: "system",
    timestamp: Date.now(),
  } satisfies ContextWithSystemEvent["messages"][number];
  // Custom messages become user messages in Pi. This transient system guidance
  // Preserves the transcript after Pi restores its prompt and tool state.
  return { messages: [...messages, message] };
}

export class ProgressTextController {
  #pending: ProgressTrigger | undefined;
  #settings: ProgressTextSettings;

  constructor(settings: ProgressTextSettings) {
    this.#pending = undefined;
    this.#settings = settings;
  }

  context(messages: ContextWithSystemEvent["messages"]): ProgressContextResult | undefined {
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

  #enabled(trigger: ProgressTrigger): boolean {
    return trigger === "compaction"
      ? this.#settings.progressTextOnCompaction
      : this.#settings.progressTextOnEffortChange;
  }
}
