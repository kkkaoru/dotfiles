import { clearTimeout, setTimeout } from "node:timers";
import { formatInterval, parseLoopCommand, type LoopCommand } from "./parser.ts";

const MIN_WAKEUP_SECONDS = 60;
const MAX_WAKEUP_SECONDS = 3600;
const MILLISECONDS_PER_SECOND = 1000;
const AUTONOMOUS_PROMPT = `Continue work already established in this conversation. Act as a steward, not an initiator: finish in-progress work, verification, or clearly authorized maintenance. Do not invent new work or perform irreversible actions without authorization. If nothing actionable remains, say so briefly and stop.`;
const SELF_PACED_GUIDANCE = `This is a self-paced loop. Perform the task now. Before ending, call loop_wakeup only when another useful check remains. Do not schedule another wakeup when the task is complete, blocked on user input, or waiting on external state that cannot be checked later.`;

type Timer = ReturnType<typeof setTimeout>;

export interface LoopContext {
  readonly isIdle: () => boolean;
  readonly ui: {
    readonly notify: (message: string, level?: "error" | "info" | "warning") => void;
    readonly setStatus: (key: string, value: string | undefined) => void;
  };
}

export interface LoopHost {
  readonly sendUserMessage: (
    content: string,
    options?: { readonly deliverAs?: "followUp"; readonly expandPromptTemplates?: boolean },
  ) => void;
}

export interface Scheduler {
  readonly clearTimeout: (timer: Timer) => void;
  readonly now: () => number;
  readonly setTimeout: (callback: () => void, delayMs: number) => Timer;
}

interface LoopJob {
  readonly id: number;
  readonly intervalMs?: number;
  readonly nextRunAt: number;
  readonly prompt: string;
  readonly reason: string;
  readonly timer: Timer;
}

interface ScheduleInput {
  readonly delayMs: number;
  readonly intervalMs?: number;
  readonly prompt: string;
  readonly reason: string;
}

export interface WakeupInput {
  readonly delaySeconds: number;
  readonly prompt: string;
  readonly reason: string;
}

export interface WakeupResult {
  readonly id: number;
  readonly scheduledInSeconds: number;
}

export const SYSTEM_SCHEDULER: Scheduler = {
  clearTimeout: (timer: Timer): void => clearTimeout(timer),
  now: (): number => Date.now(),
  setTimeout: (callback: () => void, delayMs: number): Timer => setTimeout(callback, delayMs),
};

function commandPrompt(prompt: string): string {
  const task: string = prompt.length === 0 ? AUTONOMOUS_PROMPT : prompt;
  return `${SELF_PACED_GUIDANCE}\n\nTask:\n${task}`;
}

function validateWakeup({ delaySeconds, prompt, reason }: WakeupInput): void {
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

export class LoopRuntime {
  readonly #host: LoopHost;
  readonly #jobs = new Map<number, LoopJob>();
  readonly #scheduler: Scheduler;
  #context: LoopContext | undefined;
  #nextId = 1;

  constructor(host: LoopHost, scheduler: Scheduler = SYSTEM_SCHEDULER) {
    this.#host = host;
    this.#scheduler = scheduler;
  }

  setContext(context: LoopContext): void {
    this.#context = context;
    this.#updateStatus();
  }

  command(args: string, context: LoopContext): void {
    this.setContext(context);
    const command: LoopCommand = parseLoopCommand(args);
    if (command.kind === "list") {
      this.#list(context);
      return;
    }
    if (command.kind === "clear") {
      const count: number = this.clear();
      context.ui.notify(`Cleared ${String(count)} loop job(s).`, "info");
      return;
    }
    this.#start(command, context);
  }

  wakeup(input: WakeupInput, context: LoopContext): WakeupResult {
    validateWakeup(input);
    this.setContext(context);
    const delayMs: number = input.delaySeconds * MILLISECONDS_PER_SECOND;
    const job: LoopJob = this.#schedule({
      delayMs,
      prompt: input.prompt.trim(),
      reason: input.reason.trim(),
    });
    return { id: job.id, scheduledInSeconds: input.delaySeconds };
  }

  clear(): number {
    const count: number = this.#jobs.size;
    [...this.#jobs.values()].map((job: LoopJob): void => this.#scheduler.clearTimeout(job.timer));
    this.#jobs.clear();
    this.#updateStatus();
    return count;
  }

  #start(command: Extract<LoopCommand, { readonly kind: "start" }>, context: LoopContext): void {
    const prompt: string = command.prompt.length === 0 ? AUTONOMOUS_PROMPT : command.prompt;
    if (command.intervalMs === undefined) {
      this.#send(commandPrompt(command.prompt));
      context.ui.notify("Started a self-paced loop.", "info");
      return;
    }
    const job: LoopJob = this.#schedule({
      delayMs: command.intervalMs,
      intervalMs: command.intervalMs,
      prompt,
      reason: `Recurring every ${formatInterval(command.intervalMs)}`,
    });
    this.#send(prompt);
    context.ui.notify(
      `Started loop #${String(job.id)} every ${formatInterval(command.intervalMs)} (session-scoped).`,
      "info",
    );
  }

  #schedule(input: ScheduleInput): LoopJob {
    const id: number = this.#nextId;
    this.#nextId += 1;
    const timer: Timer = this.#scheduler.setTimeout((): void => this.#fire(id), input.delayMs);
    const common = {
      id,
      nextRunAt: this.#scheduler.now() + input.delayMs,
      prompt: input.prompt,
      reason: input.reason,
      timer,
    };
    const job: LoopJob =
      input.intervalMs === undefined ? common : { ...common, intervalMs: input.intervalMs };
    this.#jobs.set(id, job);
    this.#updateStatus();
    return job;
  }

  #fire(id: number): void {
    const job: LoopJob | undefined = this.#jobs.get(id);
    if (job === undefined) {
      return;
    }
    this.#jobs.delete(id);
    this.#send(job.prompt);
    if (job.intervalMs !== undefined) {
      this.#schedule({
        delayMs: job.intervalMs,
        intervalMs: job.intervalMs,
        prompt: job.prompt,
        reason: job.reason,
      });
    }
    this.#updateStatus();
  }

  #send(prompt: string): void {
    const context: LoopContext | undefined = this.#context;
    const options: { readonly deliverAs?: "followUp" } =
      context?.isIdle() === false ? { deliverAs: "followUp" } : {};
    this.#host.sendUserMessage(prompt, options);
  }

  #list(context: LoopContext): void {
    if (this.#jobs.size === 0) {
      context.ui.notify("No loop jobs are scheduled.", "info");
      return;
    }
    const jobs: string = [...this.#jobs.values()]
      .map(
        (job: LoopJob): string =>
          `#${String(job.id)} in ${formatInterval(Math.max(0, job.nextRunAt - this.#scheduler.now()))}: ${job.reason}`,
      )
      .join("\n");
    context.ui.notify(jobs, "info");
  }

  #updateStatus(): void {
    this.#context?.ui.setStatus(
      "loop",
      this.#jobs.size === 0 ? undefined : `loop: ${String(this.#jobs.size)}`,
    );
  }
}
