// This TypeScript file is executed with Bun.
import { formatLocalTimestamp } from "./policy.ts";
import type { TmuxLaunch } from "./tmux.ts";
import type { Completion } from "./waiter.ts";

const AGENT_BUSY_ERROR = "Agent is already processing a prompt";
const MAX_COMPLETION_IDENTITY_CHARACTERS = 160;
const MAX_OVERDUE_BATCH_SIZE = 20;
const SETTLED_DELIVERY_DELAY_MS = 0;
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

function completionIdentity(command: string): string {
  return command.replaceAll(/\s+/gu, " ").trim().slice(0, MAX_COMPLETION_IDENTITY_CHARACTERS);
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

function overduePrompt(launches: readonly TmuxLaunch[]): string {
  return [
    `tmux overdue check-in: ${String(launches.length)} task(s) exceeded their estimated duration and have not completed. They are still tracked, not failed or stopped.`,
    "Inspect the logs and process state now. For an open-ended watcher, finish the observation and stop only that specific job when appropriate. For useful ongoing work, arrange a bounded next check. Do not just repeat this notice, launch a duplicate, or wait forever for an exit-status file.",
    ...launches.map((launch: TmuxLaunch): string =>
      [
        `task: ${completionIdentity(launch.taskCommand)}`,
        `tmux socket: ${launch.socketName}; session: ${launch.sessionName}`,
        `log: ${launch.logPath}`,
        `status: ${launch.statusPath}`,
      ].join("\n"),
    ),
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
  #settledDelivery: SettledDelivery | undefined;

  constructor(host: CompletionDeliveryHost, options?: CompletionDeliveryOptions) {
    this.#host = host;
    this.#onDelivered = options?.onDelivered ?? ((): void => undefined);
  }

  complete(completion: Completion): void {
    this.#pendingOverdue.delete(completion.launch.completionChannel);
    if (this.#compacting || this.#context?.isIdle() === false) {
      this.#defer(completion);
      return;
    }
    if (!this.#deliver([completion], [])) {
      this.#defer(completion);
    }
  }

  overdue(launches: readonly TmuxLaunch[]): void {
    launches.map((launch: TmuxLaunch): Map<string, TmuxLaunch> =>
      this.#pendingOverdue.set(launch.completionChannel, launch),
    );
    this.#flushIfIdle();
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
    this.#removeOverdue(overdue);
    // Context injection reaches the next model call without waiting for agent_settled.
    // Avoid stale steering prompts during a long-running tool or compaction.
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

  hasPending(): boolean {
    return this.#pending.length > 0 || this.#pendingOverdue.size > 0;
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
    this.#flushIfIdle();
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
    this.#flushIfIdle();
  }

  clear(): void {
    this.#cancelSettledDelivery();
    this.#compacting = false;
    this.#pending = [];
    this.#pendingOverdue.clear();
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

  #flushIfIdle(): void {
    if (
      this.#compacting ||
      this.#context?.isIdle() === false ||
      (this.#pending.length === 0 && this.#pendingOverdue.size === 0)
    ) {
      return;
    }
    const pending: readonly Completion[] = this.#pending;
    const overdue: readonly TmuxLaunch[] = this.#overdueBatch();
    if (this.#deliver(pending, overdue)) {
      this.#pending = [];
      this.#removeOverdue(overdue);
    }
    this.#updateStatus();
  }

  #overdueBatch(): readonly TmuxLaunch[] {
    return [...this.#pendingOverdue.values()].slice(0, MAX_OVERDUE_BATCH_SIZE);
  }

  #removeOverdue(launches: readonly TmuxLaunch[]): void {
    launches.map((launch: TmuxLaunch): boolean =>
      this.#pendingOverdue.delete(launch.completionChannel),
    );
  }

  #notifyCompletion(completion: Completion): void {
    this.#context?.ui.notify(
      completionName(completion),
      completion.exitCode === 0 ? "info" : "warning",
    );
  }

  #defer(completion: Completion): void {
    this.#pending.push(completion);
    this.#notifyCompletion(completion);
    this.#updateStatus();
  }

  #updateStatus(): void {
    this.#context?.ui.setStatus("tmux-completion", undefined);
    this.#context?.ui.setWidget?.("tmux-completions", undefined);
  }
}
