// This TypeScript file is executed with Bun.
import { tmpdir } from "node:os";
import path from "node:path";
import process from "node:process";
import {
  type Completion,
  type CompletionEvents,
  CompletionWaiter,
  type StatusOperations,
} from "./waiter.ts";

const LONG_RUNNING_TIMEOUT_SECONDS = 120;
const LONG_RUNNING_COMMAND = /(?:^|[;&|]\s*)(?:gh\s+run\s+watch|tail\s+-f\b|watch\b)/imu;
const TMUX_COMMAND = /(?:^|\s)tmux(?:\s|$)/iu;
export const TMUX_LAUNCH_TIMEOUT_MILLISECONDS = 30_000;
export const TMUX_LAUNCH_TIMEOUT_SECONDS = 30;

export interface MutableBashInput {
  command: string;
  timeout?: number;
}

export interface TmuxLaunch {
  readonly command: string;
  readonly completionChannel: string;
  readonly logPath: string;
  readonly sessionName: string;
  readonly statusPath: string;
  readonly submittedAt: string;
  readonly taskCommand: string;
}

export interface TmuxRuntimeOptions {
  readonly events?: CompletionEvents;
  readonly onComplete: (completion: Completion) => void;
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

export function createTmuxLaunch(command: string, id: number): TmuxLaunch {
  const sessionName = `pi-tmux-${String(process.pid)}-${String(id)}`;
  const completionChannel = `${sessionName}-complete`;
  const outputDirectory = path.join(tmpdir(), sessionName);
  const logPath = path.join(outputDirectory, "output.log");
  const statusPath = path.join(outputDirectory, "exit-status");
  const submittedAt: string = new Date().toISOString();
  const detachedScript = `(${command}) > ${shellQuote(logPath)} 2>&1\nexit_code=$?\nprintf '%s\\n' "$exit_code" > ${shellQuote(statusPath)}\ntmux wait-for -S ${shellQuote(completionChannel)}`;
  const message = launchMessage({ logPath, sessionName, statusPath });
  const launchCommand = `mkdir -p ${shellQuote(outputDirectory)} && tmux new-session -d -s ${shellQuote(sessionName)} -- sh -lc ${shellQuote(detachedScript)} && printf '%s\\n' ${shellQuote(message)}`;
  return {
    command: launchCommand,
    completionChannel,
    logPath,
    sessionName,
    statusPath,
    submittedAt,
    taskCommand: command,
  };
}

export function shouldDetachBash(input: MutableBashInput): boolean {
  const hasLongTimeout =
    input.timeout !== undefined && input.timeout >= LONG_RUNNING_TIMEOUT_SECONDS;
  return (
    !TMUX_COMMAND.test(input.command) &&
    (hasLongTimeout || LONG_RUNNING_COMMAND.test(input.command))
  );
}

export class TmuxRuntime {
  readonly #waiter: CompletionWaiter;
  #nextId = 1;

  constructor(options: TmuxRuntimeOptions) {
    this.#waiter = new CompletionWaiter(options);
  }

  createLaunch(command: string): TmuxLaunch {
    const launch: TmuxLaunch = createTmuxLaunch(command, this.#nextId);
    this.#nextId += 1;
    return launch;
  }

  rewriteLongBash(input: MutableBashInput): TmuxLaunch | undefined {
    if (!shouldDetachBash(input)) {
      return undefined;
    }
    const launch: TmuxLaunch = this.createLaunch(input.command);
    input.command = launch.command;
    input.timeout = TMUX_LAUNCH_TIMEOUT_SECONDS;
    return launch;
  }

  trackLaunch(launch: TmuxLaunch): void {
    this.#waiter.track(launch);
  }

  clear(): void {
    this.#waiter.clear();
  }
}
