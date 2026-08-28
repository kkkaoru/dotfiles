// This TypeScript file is executed with Bun.
import { formatLocalTimestamp } from "./policy.ts";
import type { Completion } from "./waiter.ts";

const AGENT_BUSY_ERROR = "Agent is already processing a prompt";
const MAX_COMPLETION_IDENTITY_CHARACTERS = 160;
const FOLLOW_UP_OPTIONS: CompletionDeliveryMessageOptions = { deliverAs: "followUp" };

type CompletionDeliveryResult = "delivered" | "queued" | "retry";

export interface CompletionDeliveryMessageOptions {
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
  readonly sendUserMessage: (content: string, options?: CompletionDeliveryMessageOptions) => void;
}

export interface CompletionDeliveryOptions {
  readonly onDelivered?: (completion: Completion) => void;
}

function completionIdentity(command: string): string {
  return command.replaceAll(/\s+/gu, " ").trim().slice(0, MAX_COMPLETION_IDENTITY_CHARACTERS);
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
  const failure: string =
    completion.exitCode === 0 ? "" : ` | command_exit=${String(completion.exitCode)}`;
  return `${submittedAt} → ${completedAt}${failure} | ${completionIdentity(completion.launch.taskCommand)}`;
}

function completionPrompt(completion: Completion): string {
  return `${completionName(completion)}\nlog: ${completion.launch.logPath}\nstatus: ${completion.launch.statusPath}`;
}

function isAgentBusyError(error: unknown): boolean {
  return error instanceof Error && error.message.includes(AGENT_BUSY_ERROR);
}

export function wakePiOnCompletion(host: CompletionDeliveryHost, completion: Completion): void {
  host.sendUserMessage(completionPrompt(completion));
}

export class CompletionDelivery {
  #compacting = false;
  #context: CompletionDeliveryContext | undefined;
  readonly #host: CompletionDeliveryHost;
  readonly #onDelivered: (completion: Completion) => void;
  #pending: Completion[] = [];

  constructor(host: CompletionDeliveryHost, options?: CompletionDeliveryOptions) {
    this.#host = host;
    this.#onDelivered = options?.onDelivered ?? ((): void => undefined);
  }

  complete(completion: Completion): void {
    if (this.#compacting) {
      this.#pending.push(completion);
      this.#showPending(completion);
      return;
    }
    const result: CompletionDeliveryResult = this.#deliver(
      [completion],
      this.#context?.isIdle() === false,
    );
    if (result === "retry") {
      this.#pending.push(completion);
      this.#showPending(completion);
      return;
    }
    if (result === "queued") {
      this.#notifyCompletion(completion);
    }
    this.#updateStatus();
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

  agentSettled(context: CompletionDeliveryContext): void {
    this.setContext(context);
    this.#flushIfIdle();
  }

  clear(): void {
    this.#compacting = false;
    this.#pending = [];
    this.#updateStatus();
  }

  #deliver(completions: readonly Completion[], queueAsFollowUp: boolean): CompletionDeliveryResult {
    const prompt: string = completions
      .map((completion: Completion): string => completionPrompt(completion))
      .join("\n\n");
    try {
      this.#host.sendUserMessage(prompt, queueAsFollowUp ? FOLLOW_UP_OPTIONS : undefined);
    } catch (error: unknown) {
      if (!isAgentBusyError(error)) {
        throw error;
      }
      if (queueAsFollowUp) {
        return "retry";
      }
      return this.#deliver(completions, true);
    }
    completions.map((completion: Completion): void => this.#onDelivered(completion));
    return queueAsFollowUp ? "queued" : "delivered";
  }

  #flushIfIdle(): void {
    if (this.#compacting || this.#context?.isIdle() === false || this.#pending.length === 0) {
      return;
    }
    const pending: readonly Completion[] = this.#pending;
    const result: CompletionDeliveryResult = this.#deliver(pending, false);
    if (result !== "retry") {
      this.#pending = [];
    }
    if (result === "queued") {
      pending.map((completion: Completion): void => this.#notifyCompletion(completion));
    }
    this.#updateStatus();
  }

  #notifyCompletion(completion: Completion): void {
    this.#context?.ui.notify(
      completionName(completion),
      completion.exitCode === 0 ? "info" : "warning",
    );
  }

  #showPending(completion: Completion): void {
    this.#notifyCompletion(completion);
    this.#updateStatus();
  }

  #updateStatus(): void {
    this.#context?.ui.setStatus(
      "tmux-completion",
      this.#pending.length === 0 ? undefined : `tmux: ${String(this.#pending.length)} completed`,
    );
    this.#context?.ui.setWidget?.(
      "tmux-completions",
      this.#pending.length === 0 ? undefined : this.#pending.map(completionName),
    );
  }
}
