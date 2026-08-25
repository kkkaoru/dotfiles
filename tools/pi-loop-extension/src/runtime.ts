// This TypeScript file is executed with Bun.
import type { LoopContext, LoopHost } from "./contracts.ts";
import { clearLoopDisplay, loopListText, updateLoopDisplay } from "./display.ts";
import { namedLoopFollowUp } from "./follow-up.ts";
import {
  AUTONOMOUS_PROMPT,
  commandPrompt,
  resumedJob,
  validateWakeup,
  type WakeupInput,
} from "./helpers.ts";
import { formatInterval, parseLoopCommand, type LoopCommand } from "./parser.ts";
import { type Poller, type Scheduler, SYSTEM_SCHEDULER } from "./scheduler.ts";
import {
  type LoopJobState as LoopJob,
  persistLoopState,
  restoredPendingContinuations,
  type LoopRuntimeState,
} from "./state.ts";

export type { LoopContext, LoopHost } from "./contracts.ts";
export type { WakeupInput } from "./helpers.ts";
export type { Scheduler } from "./scheduler.ts";

const MILLISECONDS_PER_SECOND = 1000;
const POLL_INTERVAL_MS = 5000;

interface ScheduleInput {
  readonly delayMs: number;
  readonly intervalMs?: number;
  readonly prompt: string;
  readonly reason: string;
}

export interface WakeupResult {
  readonly id: number;
  readonly scheduledInSeconds: number;
}

export class LoopRuntime {
  readonly #host: LoopHost;
  #jobs = new Map<number, LoopJob>();
  readonly #scheduler: Scheduler;
  #context: LoopContext | undefined;
  #nextId = 1;
  #paused = false;
  #pendingContinuations: string[] = [];
  #runningContinuation: string | undefined;
  #poller: Poller | undefined;

  constructor(host: LoopHost, scheduler: Scheduler = SYSTEM_SCHEDULER) {
    this.#host = host;
    this.#scheduler = scheduler;
  }

  setContext(context: LoopContext): void {
    this.#context = context;
    this.#updateStatus();
  }

  restore(state: LoopRuntimeState, context: LoopContext): void {
    this.#stopPoller();
    this.#context = context;
    this.#jobs = new Map(
      state.jobs.map((job: LoopJob): readonly [number, LoopJob] => [job.id, job]),
    );
    this.#nextId = state.nextId;
    this.#paused = state.paused;
    this.#pendingContinuations = [...restoredPendingContinuations(state)];
    this.#runningContinuation = state.runningContinuation;
    if (!this.#paused) {
      this.#poll();
      this.#ensurePoller();
    }
    this.#updateStatus();
    if (context.isIdle() && this.#pendingContinuations.length > 0) {
      this.agentSettled(context);
    }
  }

  shutdown(): void {
    this.#stopPoller();
    clearLoopDisplay(this.#context?.ui);
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
    this.#pendingContinuations = [];
    this.#runningContinuation = undefined;
    this.#persist();
    this.#stopPoller();
    this.#updateStatus();
    return count;
  }

  continueAfterCompaction(willRetry: boolean, context: LoopContext): void {
    this.setContext(context);
    if (willRetry || this.#runningContinuation === undefined || this.#jobs.size > 0) {
      return;
    }
    if (context.isIdle()) {
      this.#host.sendUserMessage(this.#runningContinuation);
    } else {
      this.#queue(this.#runningContinuation);
    }
    context.ui.notify("Continuing loop after compaction.", "info");
  }

  agentSettled(context: LoopContext): void {
    this.setContext(context);
    if (this.#pendingContinuations.length === 0) {
      this.#runningContinuation = undefined;
      this.#persist();
      return;
    }
    const continuations: string = this.#pendingContinuations.join("\n\n");
    this.#pendingContinuations = [];
    this.#persist();
    this.#host.sendUserMessage(continuations);
    this.#updateStatus();
  }

  #start(command: Extract<LoopCommand, { readonly kind: "start" }>, context: LoopContext): void {
    const prompt: string = command.prompt.length === 0 ? AUTONOMOUS_PROMPT : command.prompt;
    if (command.intervalMs === undefined) {
      const message: string = commandPrompt(command.prompt);
      const identity = `self-paced | ${command.prompt.length === 0 ? "continue established work" : command.prompt}`;
      const now: number = this.#scheduler.now();
      this.#send(message, identity, now, now);
      context.ui.notify("Started a self-paced loop.", "info");
      return;
    }
    const job: LoopJob = this.#schedule({
      delayMs: command.intervalMs,
      intervalMs: command.intervalMs,
      prompt,
      reason: `Recurring every ${formatInterval(command.intervalMs)}`,
    });
    this.#send(prompt, `#${String(job.id)} | ${job.reason}`, job.submittedAt, job.submittedAt);
    context.ui.notify(
      `Started loop #${String(job.id)} every ${formatInterval(command.intervalMs)} (session-scoped).`,
      "info",
    );
  }

  #schedule(input: ScheduleInput): LoopJob {
    const id: number = this.#nextId;
    this.#nextId += 1;
    const now: number = this.#scheduler.now();
    const common = {
      id,
      nextRunAt: now + input.delayMs,
      submittedAt: now,
      prompt: input.prompt,
      reason: input.reason,
    };
    const recurring: LoopJob =
      input.intervalMs === undefined ? common : { ...common, intervalMs: input.intervalMs };
    const job: LoopJob = this.#paused ? { ...recurring, remainingMs: input.delayMs } : recurring;
    this.#jobs.set(id, job);
    this.#persist();
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
    this.#send(job.prompt, `#${String(job.id)} | ${job.reason}`, job.submittedAt, now);
    if (job.intervalMs === undefined) {
      this.#jobs.delete(job.id);
    } else {
      this.#jobs.set(job.id, {
        ...job,
        nextRunAt: now + job.intervalMs,
        submittedAt: now,
      });
    }
    if (this.#jobs.size === 0) {
      this.#stopPoller();
    }
    this.#persist();
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
    this.#persist();
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
    this.#persist();
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

  #send(prompt: string, identity: string, submittedAt: number, completedAt: number): void {
    this.#runningContinuation = namedLoopFollowUp({ completedAt, identity, prompt, submittedAt });
    this.#persist();
    if (this.#context?.isIdle() === false) {
      this.#queue(this.#runningContinuation);
      return;
    }
    this.#host.sendUserMessage(completedAt === submittedAt ? prompt : this.#runningContinuation);
  }

  #queue(continuation: string): void {
    if (this.#pendingContinuations.includes(continuation)) {
      return;
    }
    this.#pendingContinuations.push(continuation);
    this.#persist();
    this.#context?.ui.notify(continuation.split("\n", 1)[0] ?? continuation, "info");
    this.#updateStatus();
  }

  #list(context: LoopContext): void {
    if (this.#jobs.size === 0) {
      context.ui.notify("No loop jobs are scheduled.", "info");
      return;
    }
    context.ui.notify(
      loopListText([...this.#jobs.values()], this.#paused, this.#scheduler.now()),
      "info",
    );
  }

  #persist(): void {
    persistLoopState(this.#host.appendEntry, {
      jobs: [...this.#jobs.values()],
      nextId: this.#nextId,
      paused: this.#paused,
      pendingContinuations: this.#pendingContinuations,
      runningContinuation: this.#runningContinuation,
    });
  }

  #updateStatus(): void {
    updateLoopDisplay({
      jobs: [...this.#jobs.values()],
      now: this.#scheduler.now(),
      paused: this.#paused,
      pendingContinuations: this.#pendingContinuations,
      ui: this.#context?.ui,
    });
  }
}
