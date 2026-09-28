// This TypeScript file is executed with Bun.
import { formatLocalTimestamp } from "./policy.ts";
import { completionIdentity, inspectedLogPath, overduePrompt } from "./overdue-inspection.ts";
import type { TmuxLaunch } from "./tmux.ts";
import type { Completion } from "./waiter.ts";

const AGENT_BUSY_ERROR = "Agent is already processing a prompt";
const MAX_OVERDUE_BATCH_SIZE = 20;
const SETTLED_DELIVERY_DELAY_MS = 0;
// Deferred completions must not wait forever for agent_settled or compaction events.
// A bounded retry flush delivers them seconds later without manual reloads or retries.
const RETRY_FLUSH_DELAY_MS = 5000;
type SettledDelivery = ReturnType<typeof globalThis.setTimeout>;
const COMPLETION_DELIVERY_OPTIONS: UserMessageDeliveryOptions = { deliverAs: "followUp" };

export interface UserMessageDeliveryOptions {
  readonly deliverAs: "followUp";
}

export interface CompletionDeliveryContext {
  readonly isIdle: () => boolean;
  readonly sessionManager?: {
    readonly getEntries: () => readonly unknown[];
    readonly getSessionId: () => string;
  };
  readonly ui: {
    readonly notify: (message: string, level?: "error" | "info" | "warning") => void;
    readonly setStatus: (key: string, value: string | undefined) => void;
    readonly setWidget?: (key: string, lines: readonly string[] | undefined) => void;
  };
}

export interface CompletionDeliveryHost {
  readonly sendUserMessage: (content: string, options?: UserMessageDeliveryOptions) => void;
}

export interface CompletionDeliveryOptions {
  readonly onDelivered?: (completion: Completion) => void;
}

function completionFailure(completion: Completion): string {
  if (completion.orphaned === true) {
    return " | orphaned";
  }
  return completion.exitCode === 0 ? "" : ` | command_exit=${String(completion.exitCode)}`;
}

function completionName(completion: Completion): string {
  const submittedDate = new Date(completion.launch.submittedAt);
  const completedDate = new Date(completion.completedAt);
  const spansDates =
    submittedDate.getFullYear() !== completedDate.getFullYear() ||
    submittedDate.getMonth() !== completedDate.getMonth() ||
    submittedDate.getDate() !== completedDate.getDate();
  const format = spansDates ? "submitted" : "completed";
  const submittedAt: string = formatLocalTimestamp(submittedDate, format);
  const completedAt: string = formatLocalTimestamp(completedDate, format);
  const failure: string = completionFailure(completion);
  return `${submittedAt} → ${completedAt}${failure} | ${completionIdentity(completion.launch.taskCommand)}`;
}

function completionPrompt(completion: Completion): string {
  return `${completionName(completion)}\nlog: ${completion.launch.logPath}\nstatus: ${completion.launch.statusPath}`;
}

function completionTimestamp(completion: Completion): number {
  return Date.parse(completion.completedAt);
}

function latestCompletion(completions: readonly Completion[]): Completion {
  const latestTimestamp: number = Math.max(
    ...completions.map((completion: Completion): number => completionTimestamp(completion)),
  );
  const latest: Completion | undefined = completions.findLast(
    (completion: Completion): boolean => completionTimestamp(completion) === latestTimestamp,
  );
  if (latest === undefined) {
    throw new Error("Cannot deliver an empty completion batch");
  }
  return latest;
}

function deliveryPrompt(completions: readonly Completion[]): string {
  if (completions.length === 1) {
    return completionPrompt(latestCompletion(completions));
  }
  const failedCount: number = completions.filter(
    (completion: Completion): boolean => completion.exitCode !== 0,
  ).length;
  const succeededCount: number = completions.length - failedCount;
  return [
    `tmux completion batch: ${String(completions.length)} tasks finished while Pi was busy (${String(succeededCount)} succeeded, ${String(failedCount)} failed or orphaned).`,
    `Earlier completion details coalesced: ${String(completions.length - 1)}. Their artifacts remain under the same Pi tmux session namespace; inspect them only if still relevant.`,
    `latest completion:\n${completionPrompt(latestCompletion(completions))}`,
  ].join("\n");
}

function isAgentBusyError(error: unknown): boolean {
  return error instanceof Error && error.message.includes(AGENT_BUSY_ERROR);
}

export function wakePiOnCompletion(host: CompletionDeliveryHost, completion: Completion): void {
  host.sendUserMessage(completionPrompt(completion), COMPLETION_DELIVERY_OPTIONS);
}

export class CompletionDelivery {
  #compacting = false;
  #context: CompletionDeliveryContext | undefined;
  readonly #host: CompletionDeliveryHost;
  readonly #onDelivered: (completion: Completion) => void;
  #pending: Completion[] = [];
  readonly #pendingOverdue = new Map<string, TmuxLaunch>();
  readonly #sentOverdue = new Set<string>();
  #retryDelivery: SettledDelivery | undefined;
  #settledDelivery: SettledDelivery | undefined;

  constructor(host: CompletionDeliveryHost, options?: CompletionDeliveryOptions) {
    this.#host = host;
    this.#onDelivered = options?.onDelivered ?? ((): void => undefined);
  }

  complete(completion: Completion): void {
    this.#pendingOverdue.delete(completion.launch.completionChannel);
    this.#sentOverdue.delete(completion.launch.completionChannel);
    if (this.#compacting || this.#context?.isIdle() === false) {
      this.#defer(completion);
      return;
    }
    if (!this.#deliver([completion], [])) {
      this.#defer(completion);
    }
  }

  overdue(launches: readonly TmuxLaunch[]): void {
    launches.map((launch: TmuxLaunch): Map<string, TmuxLaunch> => {
      this.#sentOverdue.delete(launch.completionChannel);
      return this.#pendingOverdue.set(launch.completionChannel, launch);
    });
    this.#flushPending(false);
    this.#scheduleRetryFlush();
  }

  injectOverdue(event: unknown): { messages: unknown[] } | undefined {
    if (
      this.#compacting ||
      this.#pendingOverdue.size === 0 ||
      typeof event !== "object" ||
      event === null ||
      !("messages" in event) ||
      !Array.isArray(event.messages)
    ) {
      return undefined;
    }
    const messages: readonly unknown[] = event.messages;
    const overdue: readonly TmuxLaunch[] = this.#overdueBatch();
    // Inclusion in a provider request is not evidence of inspection.
    // Keep notices through ignored/failed calls; rotate batches to avoid starvation.
    this.#removeOverdue(overdue);
    overdue.map((launch: TmuxLaunch): Map<string, TmuxLaunch> =>
      this.#pendingOverdue.set(launch.completionChannel, launch),
    );
    return {
      messages: [
        ...messages,
        {
          role: "custom",
          customType: "tmux-overdue",
          content: overduePrompt(overdue),
          display: false,
          timestamp: Date.now(),
        },
      ],
    };
  }

  inspectedLog(event: unknown): readonly TmuxLaunch[] {
    const path: string | undefined = inspectedLogPath(event);
    const inspected = [...this.#pendingOverdue.values()].filter(
      (launch: TmuxLaunch): boolean => launch.logPath === path,
    );
    inspected.map((launch: TmuxLaunch): boolean => {
      this.#sentOverdue.delete(launch.completionChannel);
      return this.#pendingOverdue.delete(launch.completionChannel);
    });
    return inspected;
  }

  hasPending(): boolean {
    return this.#pending.length > 0 || this.#pendingOverdue.size > 0;
  }

  /** Session names whose completion or overdue notice is still queued locally. */
  pendingTaskNames(): readonly string[] {
    const names = new Set<string>();
    for (const completion of this.#pending) {
      names.add(completion.launch.sessionName);
    }
    for (const launch of this.#pendingOverdue.values()) {
      names.add(launch.sessionName);
    }
    return [...names];
  }

  setContext(context: CompletionDeliveryContext): void {
    this.#context = context;
  }

  beforeCompaction(context?: CompletionDeliveryContext): void {
    this.#compacting = true;
    if (context !== undefined) {
      this.setContext(context);
    }
  }

  afterCompaction(context?: CompletionDeliveryContext): void {
    this.#compacting = false;
    if (context !== undefined) {
      this.setContext(context);
    }
    this.#flushPending(false);
  }

  deferAfterCompaction(context?: CompletionDeliveryContext): void {
    if (context !== undefined) {
      this.setContext(context);
    }
    this.#scheduleSettledDelivery((): void => this.afterCompaction(context));
  }

  deferAgentSettled(context: CompletionDeliveryContext): void {
    this.setContext(context);
    this.#scheduleSettledDelivery((): void => this.agentSettled(context));
  }

  agentSettled(context: CompletionDeliveryContext): void {
    this.setContext(context);
    // Settled delivery must not wait for full idleness; followUp queues behind the current run.
    this.#flushPending(true);
  }

  clear(): void {
    this.#cancelSettledDelivery();
    this.#cancelRetryFlush();
    this.#compacting = false;
    this.#pending = [];
    this.#pendingOverdue.clear();
    this.#sentOverdue.clear();
    this.#updateStatus();
  }

  #deliver(completions: readonly Completion[], overdue: readonly TmuxLaunch[]): boolean {
    try {
      const prompt: string = [
        completions.length === 0 ? "" : deliveryPrompt(completions),
        overdue.length === 0 ? "" : overduePrompt(overdue),
      ]
        .filter(Boolean)
        .join("\n\n");
      this.#host.sendUserMessage(prompt, COMPLETION_DELIVERY_OPTIONS);
      completions.map((completion: Completion): undefined => {
        try {
          this.#onDelivered(completion);
        } catch {
          // The message is accepted already; bookkeeping failure must not deliver it twice.
        }
        return undefined;
      });
      return true;
    } catch (error: unknown) {
      if (isAgentBusyError(error)) {
        return false;
      }
      throw error;
    }
  }

  #cancelSettledDelivery(): void {
    if (this.#settledDelivery === undefined) {
      return;
    }
    globalThis.clearTimeout(this.#settledDelivery);
    this.#settledDelivery = undefined;
  }

  #scheduleSettledDelivery(callback: () => void): void {
    if (this.#settledDelivery !== undefined) {
      return;
    }
    this.#settledDelivery = globalThis.setTimeout((): void => {
      this.#settledDelivery = undefined;
      callback();
    }, SETTLED_DELIVERY_DELAY_MS);
  }

  #scheduleRetryFlush(): void {
    if (this.#retryDelivery !== undefined || !this.#hasUndelivered()) {
      return;
    }
    this.#retryDelivery = globalThis.setTimeout((): void => {
      this.#retryDelivery = undefined;
      // Unlike settled delivery, a timer tick carries no run-completion signal and only flushes while idle.
      this.#flushPending(false);
      this.#scheduleRetryFlush();
    }, RETRY_FLUSH_DELAY_MS);
  }

  #cancelRetryFlush(): void {
    if (this.#retryDelivery === undefined) {
      return;
    }
    globalThis.clearTimeout(this.#retryDelivery);
    this.#retryDelivery = undefined;
  }

  #flushPending(settled: boolean): void {
    if (
      this.#compacting ||
      (this.#pending.length === 0 && this.#pendingOverdue.size === 0) ||
      (!settled && this.#context?.isIdle() === false)
    ) {
      return;
    }
    const pending: readonly Completion[] = this.#pending;
    const overdue: readonly TmuxLaunch[] = [...this.#pendingOverdue.values()]
      .filter((launch: TmuxLaunch): boolean => !this.#sentOverdue.has(launch.completionChannel))
      .slice(0, MAX_OVERDUE_BATCH_SIZE);
    if (pending.length === 0 && overdue.length === 0) {
      return;
    }
    if (this.#deliver(pending, overdue)) {
      this.#pending = [];
      overdue.map((launch: TmuxLaunch): Set<string> =>
        this.#sentOverdue.add(launch.completionChannel),
      );
    }
    this.#updateStatus();
  }

  #hasUndelivered(): boolean {
    return this.#pending.length > 0 || this.#pendingOverdue.size > this.#sentOverdue.size;
  }

  #overdueBatch(): readonly TmuxLaunch[] {
    return [...this.#pendingOverdue.values()].slice(0, MAX_OVERDUE_BATCH_SIZE);
  }

  #removeOverdue(launches: readonly TmuxLaunch[]): void {
    launches.map((launch: TmuxLaunch): boolean =>
      this.#pendingOverdue.delete(launch.completionChannel),
    );
  }

  #defer(completion: Completion): void {
    this.#pending.push(completion);
    this.#scheduleRetryFlush();
    this.#context?.ui.notify(
      completionName(completion),
      completion.exitCode === 0 ? "info" : "warning",
    );
    this.#updateStatus();
  }

  #updateStatus(): void {
    this.#context?.ui.setStatus("tmux-completion", undefined);
    this.#context?.ui.setWidget?.("tmux-completions", undefined);
  }
}
