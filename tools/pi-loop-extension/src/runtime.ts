// This TypeScript file is executed with Bun.
import { clearInterval, setInterval } from "node:timers";
import { formatInterval, parseLoopCommand, type LoopCommand } from "./parser.ts";

const MIN_WAKEUP_SECONDS = 60;
const MAX_WAKEUP_SECONDS = 3600;
const MILLISECONDS_PER_SECOND = 1000;
const POLL_INTERVAL_MS = 5000;
const AUTONOMOUS_PROMPT = `Continue work already established in this conversation. Act as a steward, not an initiator: finish in-progress work, verification, or clearly authorized maintenance. Do not invent new work or perform irreversible actions without authorization. If nothing actionable remains, say so briefly and stop.`;
const SELF_PACED_GUIDANCE = `This is a self-paced loop. Perform the task now. Before ending, call loop_wakeup only when another useful check remains. Do not schedule another wakeup when the task is complete, blocked on user input, or waiting on external state that cannot be checked later.`;

type Poller = ReturnType<typeof setInterval>;

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
  readonly clearInterval: (poller: Poller) => void;
  readonly now: () => number;
  readonly setInterval: (callback: () => void, intervalMs: number) => Poller;
}

interface LoopJob {
  readonly id: number;
  readonly intervalMs?: number;
  readonly nextRunAt: number;
  readonly prompt: string;
  readonly reason: string;
  readonly remainingMs?: number;
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
  clearInterval: (poller: Poller): void => clearInterval(poller),
  now: (): number => Date.now(),
  setInterval: (callback: () => void, intervalMs: number): Poller =>
    setInterval(callback, intervalMs),
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

function resumedJob(job: LoopJob, now: number): LoopJob {
  const nextRunAt: number = now + (job.remainingMs ?? 0);
  const common = {
    id: job.id,
    nextRunAt,
    prompt: job.prompt,
    reason: job.reason,
  };
  return job.intervalMs === undefined ? common : { ...common, intervalMs: job.intervalMs };
}

export class LoopRuntime {
  readonly #host: LoopHost;
  #jobs = new Map<number, LoopJob>();
  readonly #scheduler: Scheduler;
  #context: LoopContext | undefined;
  #nextId = 1;
  #paused = false;
  #runningPrompt: string | undefined;
  #poller: Poller | undefined;

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
    if (command.kind === "pause") {
      this.#pause(context);
      return;
    }
    if (command.kind === "resume") {
      this.#resume(context);
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
    this.#jobs.clear();
    this.#paused = false;
    this.#runningPrompt = undefined;
    this.#stopPoller();
    this.#updateStatus();
    return count;
  }

  continueAfterCompaction(willRetry: boolean, context: LoopContext): void {
    this.setContext(context);
    if (willRetry || this.#runningPrompt === undefined || this.#jobs.size > 0) {
      return;
    }
    this.#send(this.#runningPrompt);
    context.ui.notify("Continuing loop after compaction.", "info");
  }

  agentSettled(): void {
    this.#runningPrompt = undefined;
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
    const common = {
      id,
      nextRunAt: this.#scheduler.now() + input.delayMs,
      prompt: input.prompt,
      reason: input.reason,
    };
    const recurring: LoopJob =
      input.intervalMs === undefined ? common : { ...common, intervalMs: input.intervalMs };
    const job: LoopJob = this.#paused ? { ...recurring, remainingMs: input.delayMs } : recurring;
    this.#jobs.set(id, job);
    this.#ensurePoller();
    this.#updateStatus();
    return job;
  }

  #poll(): void {
    if (this.#paused) {
      return;
    }
    const now: number = this.#scheduler.now();
    [...this.#jobs.values()]
      .filter((job: LoopJob): boolean => job.nextRunAt <= now)
      .map((job: LoopJob): void => this.#fire(job, now));
  }

  #fire(job: LoopJob, now: number): void {
    this.#send(job.prompt);
    if (job.intervalMs === undefined) {
      this.#jobs.delete(job.id);
    } else {
      this.#jobs.set(job.id, { ...job, nextRunAt: now + job.intervalMs });
    }
    if (this.#jobs.size === 0) {
      this.#stopPoller();
    }
    this.#updateStatus();
  }

  #pause(context: LoopContext): void {
    if (this.#paused) {
      context.ui.notify("Loop jobs are already paused.", "info");
      return;
    }
    const now: number = this.#scheduler.now();
    this.#jobs = new Map(
      [...this.#jobs.entries()].map(
        ([id, job]: readonly [number, LoopJob]): readonly [number, LoopJob] => [
          id,
          { ...job, remainingMs: Math.max(0, job.nextRunAt - now) },
        ],
      ),
    );
    this.#paused = true;
    this.#stopPoller();
    this.#updateStatus();
    context.ui.notify(`Paused ${String(this.#jobs.size)} loop job(s).`, "info");
  }

  #resume(context: LoopContext): void {
    if (!this.#paused) {
      context.ui.notify("Loop jobs are not paused.", "info");
      return;
    }
    const now: number = this.#scheduler.now();
    this.#jobs = new Map(
      [...this.#jobs.entries()].map(
        ([id, job]: readonly [number, LoopJob]): readonly [number, LoopJob] => [
          id,
          resumedJob(job, now),
        ],
      ),
    );
    this.#paused = false;
    this.#ensurePoller();
    this.#updateStatus();
    context.ui.notify(`Resumed ${String(this.#jobs.size)} loop job(s).`, "info");
  }

  #ensurePoller(): void {
    if (this.#poller !== undefined || this.#jobs.size === 0 || this.#paused) {
      return;
    }
    this.#poller = this.#scheduler.setInterval((): void => this.#poll(), POLL_INTERVAL_MS);
  }

  #stopPoller(): void {
    if (this.#poller === undefined) {
      return;
    }
    this.#scheduler.clearInterval(this.#poller);
    this.#poller = undefined;
  }

  #send(prompt: string): void {
    this.#runningPrompt = prompt;
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
    const now: number = this.#scheduler.now();
    const jobs: string = [...this.#jobs.values()]
      .map((job: LoopJob): string => {
        const remainingMs: number = job.remainingMs ?? Math.max(0, job.nextRunAt - now);
        const state: string = this.#paused ? "paused, " : "";
        return `#${String(job.id)} ${state}in ${formatInterval(remainingMs)}: ${job.reason}`;
      })
      .join("\n");
    context.ui.notify(jobs, "info");
  }

  #updateStatus(): void {
    const paused: string = this.#paused ? " (paused)" : "";
    this.#context?.ui.setStatus(
      "loop",
      this.#jobs.size === 0 ? undefined : `loop: ${String(this.#jobs.size)}${paused}`,
    );
  }
}
