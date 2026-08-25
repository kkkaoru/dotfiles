// This TypeScript file is executed with Bun.
import { formatLocalTimestamp } from "./policy.ts";
import type { Completion } from "./waiter.ts";

const MAX_COMPLETION_IDENTITY_CHARACTERS = 160;

export interface CompletionDeliveryContext {
  readonly isIdle: () => boolean;
  readonly sessionManager?: { readonly getEntries: () => readonly unknown[] };
  readonly ui: {
    readonly notify: (message: string, level?: "error" | "info" | "warning") => void;
    readonly setStatus: (key: string, value: string | undefined) => void;
    readonly setWidget?: (key: string, lines: readonly string[] | undefined) => void;
  };
}

export interface CompletionDeliveryHost {
  readonly sendUserMessage: (content: string) => void;
}

export interface CompletionDeliveryOptions {
  readonly onDelivered?: (completion: Completion) => void;
}

function completionIdentity(command: string): string {
  return command.replaceAll(/\s+/gu, " ").trim().slice(0, MAX_COMPLETION_IDENTITY_CHARACTERS);
}

function completionName(completion: Completion): string {
  const submittedAt: string = formatLocalTimestamp(
    new Date(completion.launch.submittedAt),
    "submitted",
  );
  const completedAt: string = formatLocalTimestamp(new Date(), "completed");
  const failure: string =
    completion.exitCode === 0 ? "" : ` | failed=${String(completion.exitCode)}`;
  return `${submittedAt} → ${completedAt} | tmux=${completion.launch.sessionName}${failure} | ${completionIdentity(completion.launch.taskCommand)}`;
}

function completionPrompt(completion: Completion): string {
  return `${completionName(completion)}\nlog: ${completion.launch.logPath}\nstatus: ${completion.launch.statusPath}`;
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
    if (this.#compacting || this.#context?.isIdle() === false) {
      this.#pending.push(completion);
      this.#showPending(completion);
      return;
    }
    this.#deliver([completion]);
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

  #deliver(completions: readonly Completion[]): void {
    this.#host.sendUserMessage(
      completions
        .map((completion: Completion): string => completionPrompt(completion))
        .join("\n\n"),
    );
    completions.map((completion: Completion): void => this.#onDelivered(completion));
  }

  #flushIfIdle(): void {
    if (this.#compacting || this.#context?.isIdle() === false || this.#pending.length === 0) {
      return;
    }
    const pending: readonly Completion[] = this.#pending;
    this.#deliver(pending);
    this.#pending = [];
    this.#updateStatus();
  }

  #showPending(completion: Completion): void {
    this.#context?.ui.notify(
      completionName(completion),
      completion.exitCode === 0 ? "info" : "error",
    );
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
