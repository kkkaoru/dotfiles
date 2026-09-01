// This TypeScript file is executed with Bun.
import { formatLocalTimestamp } from "./policy.ts";
import type { Completion } from "./waiter.ts";

const AGENT_BUSY_ERROR = "Agent is already processing a prompt";
const MAX_COMPLETION_IDENTITY_CHARACTERS = 160;
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
  #settledDelivery: SettledDelivery | undefined;

  constructor(host: CompletionDeliveryHost, options?: CompletionDeliveryOptions) {
    this.#host = host;
    this.#onDelivered = options?.onDelivered ?? ((): void => undefined);
  }

  complete(completion: Completion): void {
    if (this.#compacting || this.#context?.isIdle() === false) {
      this.#defer(completion);
      return;
    }
    if (!this.#deliver([completion])) {
      this.#defer(completion);
    }
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
    this.#updateStatus();
  }

  #deliver(completions: readonly Completion[]): boolean {
    try {
      this.#host.sendUserMessage(
        completions
          .map((completion: Completion): string => completionPrompt(completion))
          .join("\n\n"),
        COMPLETION_DELIVERY_OPTIONS,
      );
      completions.map((completion: Completion): void => this.#onDelivered(completion));
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
    if (this.#compacting || this.#context?.isIdle() === false || this.#pending.length === 0) {
      return;
    }
    const pending: readonly Completion[] = this.#pending;
    if (this.#deliver(pending)) {
      this.#pending = [];
    }
    this.#updateStatus();
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
