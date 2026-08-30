// This TypeScript file is executed with Bun.
import { createHash, randomUUID } from "node:crypto";
import { tmpdir } from "node:os";
import path from "node:path";
import { LAUNCH_METADATA_FILENAME, serializeLaunchMetadata } from "./persistence.ts";
import { type MutableBashInput, shouldDetachBash } from "./policy.ts";
import {
  type Completion,
  type CompletionEvents,
  CompletionWaiter,
  type StatusOperations,
} from "./waiter.ts";

export type { MutableBashInput } from "./policy.ts";
export const DEFAULT_ESTIMATED_DURATION_SECONDS = 120;
export const RECONCILIATION_INTERVAL_MILLISECONDS = 60_000;
export const TMUX_LAUNCH_TIMEOUT_MILLISECONDS = 30_000;
export const TMUX_LAUNCH_TIMEOUT_SECONDS = 30;

export interface CreateTmuxLaunchInput {
  readonly command: string;
  readonly estimatedDurationSeconds?: number;
  readonly id: number;
  readonly namespace: string;
}

export interface TmuxLaunch {
  readonly command: string;
  readonly completionChannel: string;
  readonly estimatedCompletionAt?: string;
  readonly logPath: string;
  readonly sessionName: string;
  readonly socketName: string;
  readonly statusPath: string;
  readonly submittedAt: string;
  readonly taskCommand: string;
}

export interface TmuxRuntimeOptions {
  readonly events?: CompletionEvents;
  readonly onActiveChange?: (launches: readonly TmuxLaunch[]) => void;
  readonly onComplete: (completion: Completion) => void;
  readonly onTrack?: (launch: TmuxLaunch) => void;
  readonly operations?: StatusOperations;
}

function shellQuote(value: string): string {
  return `'${value.replaceAll("'", `'"'"'`)}'`;
}

function launchMessage(launch: Pick<TmuxLaunch, "logPath" | "sessionName" | "statusPath">): string {
  return [
    "Started detached tmux command.",
    `tmux session: ${launch.sessionName}`,
    `log: ${launch.logPath}`,
    `exit status: ${launch.statusPath}`,
    "Return control now. Pi will wake automatically when the exit-status file appears.",
  ].join("\n");
}

export function tmuxSessionNamespace(sessionId: string): string {
  return createHash("sha256").update(sessionId).digest("hex").slice(0, 32);
}

export function createTmuxLaunch(input: CreateTmuxLaunchInput): TmuxLaunch {
  const { command, id, namespace } = input;
  if (!/^[a-f0-9]{32}$/u.test(namespace)) {
    throw new Error("invalid tmux session namespace");
  }
  const socketName = `pi-tmux-${namespace}`;
  const sessionName = `${socketName}-${String(id)}`;
  const completionChannel = `${sessionName}-complete`;
  const outputDirectory = path.join(tmpdir(), sessionName);
  const logPath = path.join(outputDirectory, "output.log");
  const statusPath = path.join(outputDirectory, "exit-status");
  const submittedAt: string = new Date().toISOString();
  const estimatedDurationSeconds: number =
    input.estimatedDurationSeconds ?? DEFAULT_ESTIMATED_DURATION_SECONDS;
  const estimatedCompletionAt: string = new Date(
    Date.parse(submittedAt) + estimatedDurationSeconds * 1000,
  ).toISOString();
  const detachedScript = `(${command}) > ${shellQuote(logPath)} 2>&1\nexit_code=$?\nprintf '%s\\n' "$exit_code" > ${shellQuote(statusPath)}\ntmux -L ${shellQuote(socketName)} wait-for -S ${shellQuote(completionChannel)}`;
  const message = launchMessage({ logPath, sessionName, statusPath });
  const launch: TmuxLaunch = {
    command: "",
    completionChannel,
    estimatedCompletionAt,
    logPath,
    sessionName,
    socketName,
    statusPath,
    submittedAt,
    taskCommand: command,
  };
  const metadataPath: string = path.join(outputDirectory, LAUNCH_METADATA_FILENAME);
  const launchCommand = `mkdir -p ${shellQuote(outputDirectory)} && tmux -L ${shellQuote(socketName)} new-session -d -s ${shellQuote(sessionName)} -- sh -lc ${shellQuote(detachedScript)} && printf '%s' ${shellQuote(serializeLaunchMetadata(launch))} > ${shellQuote(metadataPath)} && printf '%s\\n' ${shellQuote(message)}`;
  return { ...launch, command: launchCommand };
}

export { shouldDetachBash } from "./policy.ts";

function launchIdForNamespace(launch: TmuxLaunch, namespace: string): number | undefined {
  const match = new RegExp(`^pi-tmux-${namespace}-(\\d+)$`, "u").exec(launch.sessionName);
  if (
    launch.socketName !== `pi-tmux-${namespace}` ||
    launch.completionChannel !== `${launch.sessionName}-complete` ||
    match?.[1] === undefined
  ) {
    return undefined;
  }
  return Number(match[1]);
}

export class TmuxRuntime {
  readonly #active = new Map<string, TmuxLaunch>();
  readonly #onActiveChange: (launches: readonly TmuxLaunch[]) => void;
  readonly #onComplete: (completion: Completion) => void;
  readonly #onTrack: (launch: TmuxLaunch) => void;
  readonly #waiter: CompletionWaiter;
  #nextId = 1;
  #reconciliationTimer: NodeJS.Timeout | undefined;
  #namespace = tmuxSessionNamespace(randomUUID());

  constructor(options: TmuxRuntimeOptions) {
    this.#onActiveChange = options.onActiveChange ?? ((): void => undefined);
    this.#onComplete = options.onComplete;
    this.#onTrack = options.onTrack ?? ((): void => undefined);
    this.#waiter = new CompletionWaiter({
      ...options,
      onComplete: (completion: Completion): void => this.#complete(completion),
    });
  }

  startSession(sessionId: string): string {
    this.clear();
    this.#namespace = tmuxSessionNamespace(sessionId);
    this.#nextId = 1;
    return this.#namespace;
  }

  createLaunch(command: string, estimatedDurationSeconds?: number): TmuxLaunch {
    const launch: TmuxLaunch = createTmuxLaunch({
      command,
      ...(estimatedDurationSeconds === undefined ? {} : { estimatedDurationSeconds }),
      id: this.#nextId,
      namespace: this.#namespace,
    });
    this.#nextId += 1;
    return launch;
  }

  rewriteLongBash(input: MutableBashInput): TmuxLaunch | undefined {
    if (!shouldDetachBash(input)) {
      return undefined;
    }
    const launch: TmuxLaunch = this.createLaunch(input.command, input.timeout);
    input.command = launch.command;
    input.timeout = TMUX_LAUNCH_TIMEOUT_SECONDS;
    return launch;
  }

  trackLaunch(launch: TmuxLaunch): void {
    this.#onTrack(launch);
    this.#active.set(launch.completionChannel, launch);
    this.#notifyActiveChange();
    this.#waiter.track(launch);
  }

  restore(launches: readonly TmuxLaunch[], nextId = 1): void {
    this.#nextId = Math.max(this.#nextId, nextId);
    launches.map((launch: TmuxLaunch): boolean => {
      const id: number | undefined = launchIdForNamespace(launch, this.#namespace);
      if (id === undefined) {
        return false;
      }
      this.#nextId = Math.max(this.#nextId, id + 1);
      this.#active.set(launch.completionChannel, launch);
      this.#waiter.track(launch);
      return true;
    });
    this.#notifyActiveChange();
    this.reconcile();
  }

  reconcile(): void {
    this.#waiter.reconcile();
  }

  clear(): void {
    this.#waiter.clear();
    this.#active.clear();
    this.#notifyActiveChange();
  }

  #complete(completion: Completion): void {
    this.#active.delete(completion.launch.completionChannel);
    this.#notifyActiveChange();
    this.#onComplete(completion);
  }

  #notifyActiveChange(): void {
    this.#onActiveChange([...this.#active.values()]);
    this.#updateReconciliationTimer();
  }

  #updateReconciliationTimer(): void {
    if (this.#active.size === 0) {
      if (this.#reconciliationTimer !== undefined) {
        globalThis.clearInterval(this.#reconciliationTimer);
        this.#reconciliationTimer = undefined;
      }
      return;
    }
    if (this.#reconciliationTimer !== undefined) {
      return;
    }
    this.#reconciliationTimer = globalThis.setInterval(
      (): void => this.reconcile(),
      RECONCILIATION_INTERVAL_MILLISECONDS,
    );
    this.#reconciliationTimer.unref();
  }
}
