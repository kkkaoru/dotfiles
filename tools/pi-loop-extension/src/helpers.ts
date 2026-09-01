// This TypeScript file is executed with Bun.
import type { LoopHost, UserMessageDeliveryOptions } from "./contracts.ts";
import type { LoopJobState } from "./state.ts";

const AGENT_BUSY_ERROR = "Agent is already processing a prompt";
const USER_MESSAGE_DELIVERY_OPTIONS: UserMessageDeliveryOptions = { deliverAs: "followUp" };
const MIN_WAKEUP_SECONDS = 60;
const MAX_WAKEUP_SECONDS = 3600;
export const AUTONOMOUS_PROMPT = `Continue work already established in this conversation. Act as a steward, not an initiator: finish in-progress work, verification, or clearly authorized maintenance. Do not invent new work or perform irreversible actions without authorization. If nothing actionable remains, say so briefly and stop.`;
const SELF_PACED_GUIDANCE = `This is a self-paced loop. Perform the task now and continue through every immediately actionable step. Do not end by merely reporting remaining work. Before ending, make exactly one terminal loop decision: call loop_wakeup when another useful later check remains, or call loop_complete only when the task is complete or blocked on user input. If neither tool is called, the loop automatically continues.`;

export function trySendUserMessage(host: LoopHost, message: string): boolean {
  try {
    host.sendUserMessage(message, USER_MESSAGE_DELIVERY_OPTIONS);
    return true;
  } catch (error: unknown) {
    if (error instanceof Error && error.message.includes(AGENT_BUSY_ERROR)) {
      return false;
    }
    throw error;
  }
}

export interface WakeupInput {
  readonly delaySeconds: number;
  readonly prompt: string;
  readonly reason: string;
}

export function commandPrompt(prompt: string): string {
  const task: string = prompt.length === 0 ? AUTONOMOUS_PROMPT : prompt;
  return `${SELF_PACED_GUIDANCE}\n\nTask:\n${task}`;
}

export function validateWakeup({ delaySeconds, prompt, reason }: WakeupInput): void {
  if (!Number.isInteger(delaySeconds)) {
    throw new TypeError("delaySeconds must be an integer");
  }
  if (delaySeconds < MIN_WAKEUP_SECONDS || delaySeconds > MAX_WAKEUP_SECONDS) {
    throw new Error("delaySeconds must be between 60 and 3,600");
  }
  if (prompt.trim().length === 0 || reason.trim().length === 0) {
    throw new Error("prompt and reason must not be empty");
  }
}

export function resumedJob(job: LoopJobState, now: number): LoopJobState {
  const nextRunAt: number = now + (job.remainingMs ?? 0);
  const common = {
    id: job.id,
    nextRunAt,
    submittedAt: job.submittedAt,
    prompt: job.prompt,
    reason: job.reason,
  };
  return job.intervalMs === undefined ? common : { ...common, intervalMs: job.intervalMs };
}
