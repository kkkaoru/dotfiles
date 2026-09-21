// This TypeScript file is executed with Bun.
import type { CompleteResult, LoopContext, LoopHost } from "./contracts.ts";
import { clearLoopDisplay, updateLoopDisplay } from "./display.ts";
import { namedLoopFollowUp } from "./follow-up.ts";
import {
  commandPrompt,
  requireActiveLoop,
  trySendUserMessage,
  validateWakeup,
  type WakeupInput,
  type WakeupResult,
} from "./helpers.ts";
import { loopListMessage, pausedLoopNotice, pauseJobs, resumeJobs } from "./job-control.ts";
import { parseLoopCommand, type LoopCommand } from "./parser.ts";
import { startLoopCommand, type StartCommandScheduleInput } from "./start-command.ts";
import { type Poller, type Scheduler, SYSTEM_SCHEDULER } from "./scheduler.ts";
import { SettledDelivery } from "./settled-delivery.ts";
import { ABANDONED_LOOP_NOTICE, settleTick } from "./settled-tick.ts";
import {
  type LoopJobState as LoopJob,
  persistLoopState,
  restoredQueuedContinuations,
  type LoopRuntimeState,
} from "./state.ts";
export type { LoopContext, LoopHost } from "./contracts.ts";
export type { WakeupInput, WakeupResult } from "./helpers.ts";
export type { Scheduler } from "./scheduler.ts";
const MILLISECONDS_PER_SECOND = 1000;
const POLL_INTERVAL_MS = 5000;
export class LoopRuntime {
  readonly #host: LoopHost;
  #jobs = new Map<number, LoopJob>();
  readonly #scheduler: Scheduler;
  #context: LoopContext | undefined;
  #nextId = 1;
  #paused = false;
  #pendingContinuations: string[] = [];
  #runningContinuation: string | undefined;
  #continuedWithoutTerminal = false;
  #poller: Poller | undefined;
  readonly #settledDelivery = new SettledDelivery();

  constructor(host: LoopHost, scheduler: Scheduler = SYSTEM_SCHEDULER) {
    this.#host = host;
    this.#scheduler = scheduler;
  }

  ownsContinuation(): boolean {
    const active: boolean = this.#runningContinuation !== undefined;
    return !this.#paused && (active || this.#pendingContinuations.length + this.#jobs.size > 0);
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
    this.#pendingContinuations = [...restoredQueuedContinuations(state)];
    this.#runningContinuation = undefined;
    this.#continuedWithoutTerminal = false;
    this.#updateStatus();
    if (this.#paused) {
      context.ui.notify(
        pausedLoopNotice(this.#jobs.size, this.#pendingContinuations.length),
        "warning",
      );
      return;
    }
    this.#poll();
    this.#ensurePoller();
    if (context.isIdle() && this.#pendingContinuations.length > 0) {
      this.deferLifecycleContinuation((): void => this.agentSettled(context));
    }
  }

  shutdown(): void {
    this.#stopPoller();
    this.#settledDelivery.cancel();
    clearLoopDisplay(this.#context?.ui);
  }

  command(args: string, context: LoopContext): void {
    this.setContext(context);
    const command: LoopCommand = parseLoopCommand(args);
    if (command.kind === "list") {
      context.ui.notify(
        loopListMessage({ jobs: this.#jobs, now: this.#scheduler.now(), paused: this.#paused }),
        "info",
      );
      return;
    }
    if (command.kind === "clear") {
      context.ui.notify(`Cleared ${String(this.clear())} loop job(s).`, "info");
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
    startLoopCommand(command, {
      notify: (message: string, level: "info"): void => context.ui.notify(message, level),
      now: (): number => this.#scheduler.now(),
      schedule: (input): LoopJob => this.#schedule(input),
      send: (prompt: string, identity: string, submittedAt: number, completedAt: number): void =>
        this.#send(prompt, identity, submittedAt, completedAt),
    });
  }

  wakeup(input: WakeupInput, context: LoopContext): WakeupResult {
    validateWakeup(input);
    this.setContext(context);
    requireActiveLoop(this.#runningContinuation, "loop_wakeup");
    this.#runningContinuation = undefined;
    this.#continuedWithoutTerminal = false;
    const delayMs: number = input.delaySeconds * MILLISECONDS_PER_SECOND;
    const job: LoopJob = this.#schedule({
      delayMs,
      prompt: input.prompt.trim(),
      reason: input.reason.trim(),
    });
    return { id: job.id, scheduledInSeconds: input.delaySeconds };
  }

  complete(reason: string, context: LoopContext): CompleteResult {
    this.setContext(context);
    requireActiveLoop(this.#runningContinuation, "loop_complete");
    const normalizedReason: string = reason.trim();
    if (normalizedReason.length === 0) {
      throw new Error("reason must not be empty");
    }
    this.#runningContinuation = undefined;
    this.#continuedWithoutTerminal = false;
    this.#persist();
    this.#updateStatus();
    return { reason: normalizedReason };
  }

  clear(): number {
    const count: number = this.#jobs.size;
    this.#jobs.clear();
    this.#paused = false;
    this.#pendingContinuations = [];
    this.#runningContinuation = undefined;
    this.#continuedWithoutTerminal = false;
    this.#persist();
    this.#stopPoller();
    this.#updateStatus();
    return count;
  }

  continueAfterCompaction(willRetry: boolean, context: LoopContext): void {
    if (!willRetry) {
      this.agentSettled(context);
    }
  }

  startFromAgent(prompt: string, context: LoopContext): void {
    if (this.#paused || prompt.trim().length === 0) {
      throw new Error("A new agent loop requires a non-empty task and no paused loop.");
    }
    this.setContext(context);
    this.clear();
    const now: number = this.#scheduler.now();
    this.#runningContinuation = namedLoopFollowUp({
      completedAt: now,
      submittedAt: now,
      identity: "self-paced | agent-defined task",
      prompt: commandPrompt(prompt.trim()),
    });
    this.#persist();
  }

  deferLifecycleContinuation(callback: () => void): void {
    this.#settledDelivery.schedule(callback);
  }

  agentSettled(context: LoopContext): void {
    this.setContext(context);
    if (this.#paused || !context.isIdle()) {
      return;
    }
    settleTick(
      {
        continuedWithoutTerminal: this.#continuedWithoutTerminal,
        jobs: this.#jobs.size,
        pending: this.#pendingContinuations,
        running: this.#runningContinuation,
      },
      {
        abandon: (): void => this.#stopAbandoned(context),
        clearPending: (): void => {
          this.#pendingContinuations = [];
        },
        clearRunning: (): void => {
          this.#runningContinuation = undefined;
          this.#continuedWithoutTerminal = false;
        },
        markContinued: (): void => {
          this.#continuedWithoutTerminal = true;
        },
        notify: (message: string, level: "info" | "warning"): void => {
          context.ui.notify(message, level);
        },
        persist: (): void => this.#persist(),
        queue: (text: string): void => this.#queue(text),
        updateStatus: (): void => this.#updateStatus(),
      },
      this.#host,
    );
  }

  #schedule(input: StartCommandScheduleInput): LoopJob {
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
    const prompt: string = job.intervalMs === undefined ? commandPrompt(job.prompt) : job.prompt;
    this.#send(prompt, `#${String(job.id)} | ${job.reason}`, job.submittedAt, now);
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
    this.#jobs = pauseJobs({ jobs: this.#jobs, now: this.#scheduler.now() });
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
    this.#jobs = resumeJobs({ jobs: this.#jobs, now: this.#scheduler.now() });
    this.#paused = false;
    this.#persist();
    this.#ensurePoller();
    this.#updateStatus();
    context.ui.notify(`Resumed ${String(this.#jobs.size)} loop job(s).`, "info");
    this.deferLifecycleContinuation((): void => this.agentSettled(context));
  }

  #ensurePoller(): void {
    if (this.#poller !== undefined || this.#jobs.size === 0 || this.#paused) {
      return;
    }
    this.#poller = this.#scheduler.setInterval((): void => this.#poll(), POLL_INTERVAL_MS);
  }

  #stopPoller(): void {
    if (this.#poller !== undefined) {
      this.#scheduler.clearInterval(this.#poller);
      this.#poller = undefined;
    }
  }

  #stopAbandoned(context: LoopContext): void {
    this.#runningContinuation = undefined;
    this.#continuedWithoutTerminal = false;
    this.#persist();
    this.#updateStatus();
    context.ui.notify(ABANDONED_LOOP_NOTICE, "warning");
  }

  #send(prompt: string, identity: string, submittedAt: number, completedAt: number): void {
    this.#runningContinuation = namedLoopFollowUp({ completedAt, identity, prompt, submittedAt });
    this.#continuedWithoutTerminal = false;
    this.#persist();
    if (this.#context?.isIdle() === false) {
      this.#queue(this.#runningContinuation);
      return;
    }
    const message: string = completedAt === submittedAt ? prompt : this.#runningContinuation;
    if (!trySendUserMessage(this.#host, message)) {
      this.#queue(this.#runningContinuation);
    }
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
