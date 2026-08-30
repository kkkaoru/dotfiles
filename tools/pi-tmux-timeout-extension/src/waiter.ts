// This TypeScript file is executed with Bun.
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import type { TmuxLaunch } from "./tmux.ts";

export interface Completion {
  readonly completedAt: string;
  readonly exitCode: number;
  readonly launch: TmuxLaunch;
  readonly orphaned?: boolean;
}

export interface CompletionEvents {
  readonly subscribe: (input: CompletionSubscriptionInput) => () => void;
}

export interface CompletionSubscriptionInput {
  readonly channel: string;
  readonly onSignal: () => void;
  readonly socketName: string;
}

export interface StatusOperations {
  readonly isRunning?: (launch: TmuxLaunch) => boolean;
  readonly read: (path: string) => string;
}

export interface CompletionWaiterOptions {
  readonly events?: CompletionEvents;
  readonly onComplete: (completion: Completion) => void;
  readonly operations?: StatusOperations;
}

export interface WaitProcess {
  readonly kill: (signal: "SIGTERM") => void;
  readonly once: (event: "close", listener: (code: number | null) => void) => void;
}

type SpawnProcess = (
  command: string,
  args: readonly string[],
  options: { readonly stdio: "ignore" },
) => WaitProcess;

const SYSTEM_OPERATIONS: StatusOperations = {
  isRunning: (launch: TmuxLaunch): boolean =>
    spawnSync("tmux", ["-L", launch.socketName, "has-session", "-t", launch.sessionName], {
      stdio: "ignore",
    }).status === 0,
  read: (path: string): string => fs.readFileSync(path, "utf8"),
};

export function createCompletionEvents(spawnProcess: SpawnProcess = spawn): CompletionEvents {
  return {
    subscribe: ({ channel, onSignal, socketName }: CompletionSubscriptionInput): (() => void) => {
      const process: WaitProcess = spawnProcess("tmux", ["-L", socketName, "wait-for", channel], {
        stdio: "ignore",
      });
      process.once("close", (code: number | null): void => {
        if (code === 0) {
          onSignal();
        }
      });
      return (): void => {
        process.kill("SIGTERM");
      };
    },
  };
}

const SYSTEM_EVENTS: CompletionEvents = createCompletionEvents();

export class CompletionWaiter {
  readonly #cancellations = new Map<string, () => void>();
  readonly #events: CompletionEvents;
  readonly #launches = new Map<string, TmuxLaunch>();
  readonly #onComplete: (completion: Completion) => void;
  readonly #operations: StatusOperations;

  constructor(options: CompletionWaiterOptions) {
    this.#events = options.events ?? SYSTEM_EVENTS;
    this.#onComplete = options.onComplete;
    this.#operations = options.operations ?? SYSTEM_OPERATIONS;
  }

  track(launch: TmuxLaunch): void {
    this.cancel(launch);
    const cancel: () => void = this.#events.subscribe({
      channel: launch.completionChannel,
      onSignal: (): void => this.#complete(launch),
      socketName: launch.socketName,
    });
    this.#cancellations.set(launch.completionChannel, cancel);
    this.#launches.set(launch.completionChannel, launch);
  }

  cancel(launch: TmuxLaunch): void {
    this.#cancellations.get(launch.completionChannel)?.();
    this.#cancellations.delete(launch.completionChannel);
    this.#launches.delete(launch.completionChannel);
  }

  reconcile(): void {
    [...this.#launches.values()].map((launch: TmuxLaunch): void => this.#complete(launch));
  }

  clear(): void {
    for (const cancel of this.#cancellations.values()) {
      cancel();
    }
    this.#cancellations.clear();
    this.#launches.clear();
  }

  #complete(launch: TmuxLaunch): void {
    if (!this.#cancellations.has(launch.completionChannel)) {
      return;
    }
    const exitCode: number | undefined = this.#readExitCode(launch.statusPath);
    if (exitCode !== undefined) {
      this.#finish({ completedAt: new Date().toISOString(), exitCode, launch });
      return;
    }
    if (this.#operations.isRunning?.(launch) !== false) {
      return;
    }
    this.#finish({
      completedAt: new Date().toISOString(),
      exitCode: 255,
      launch,
      orphaned: true,
    });
  }

  #finish(completion: Completion): void {
    this.cancel(completion.launch);
    this.#onComplete(completion);
  }

  #readExitCode(statusPath: string): number | undefined {
    try {
      const status: string = this.#operations.read(statusPath).trim();
      return /^\d+$/u.test(status) ? Number(status) : undefined;
    } catch {
      return undefined;
    }
  }
}
