// This TypeScript file is executed with Bun.
import fs from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { clearInterval, setInterval } from "node:timers";

export const ARTIFACT_CLEANUP_INTERVAL_MILLISECONDS = 86_400_000;
export const ARTIFACT_CLEANUP_STAMP_FILENAME = ".pi-tmux-cleanup-stamp";
export const ARTIFACT_RETENTION_MILLISECONDS = 604_800_000;

export interface ArtifactCleanupOperations {
  readonly readDirectory: (directory: string) => Promise<readonly string[]>;
  readonly readFile: (filePath: string) => Promise<string>;
  readonly removeDirectory: (directory: string) => Promise<void>;
  readonly statMtime: (filePath: string) => Promise<number>;
  readonly writeFile: (filePath: string, content: string) => Promise<void>;
}

export interface ArtifactCleanupOptions {
  readonly nowMs?: () => number;
  readonly operations?: ArtifactCleanupOperations;
  readonly rootDirectory?: string;
}

export interface CleanupTimer {
  readonly cancel: () => void;
  readonly unref: () => void;
}

export interface CleanupScheduler {
  readonly clear: (timer: CleanupTimer) => void;
  readonly schedule: (callback: () => void, milliseconds: number) => CleanupTimer;
}

export interface ArtifactCleanerOptions extends ArtifactCleanupOptions {
  readonly scheduler?: CleanupScheduler;
}

const ARTIFACT_DIRECTORY_PATTERN = /^pi-tmux-\d+-\d+$/u;
const EXIT_STATUS_PATTERN = /^\d+\s*$/u;
export const systemArtifactCleanupOperations: ArtifactCleanupOperations = {
  readDirectory: async (directory: string): Promise<string[]> => fs.readdir(directory),
  readFile: async (filePath: string): Promise<string> => fs.readFile(filePath, "utf8"),
  removeDirectory: async (directory: string): Promise<void> => {
    await fs.rm(directory, { force: true, recursive: true });
  },
  statMtime: async (filePath: string): Promise<number> => {
    const statistics = await fs.stat(filePath);
    return statistics.mtimeMs;
  },
  writeFile: async (filePath: string, content: string): Promise<void> => {
    await fs.writeFile(filePath, content);
  },
};
const SYSTEM_SCHEDULER: CleanupScheduler = {
  clear: (timer: CleanupTimer): void => timer.cancel(),
  schedule: (callback: () => void, milliseconds: number): CleanupTimer => {
    const timer: NodeJS.Timeout = setInterval(callback, milliseconds);
    return {
      cancel: (): void => clearInterval(timer),
      unref: (): void => {
        timer.unref();
      },
    };
  },
};

async function removeExpiredDirectory(
  directory: string,
  nowMs: number,
  operations: ArtifactCleanupOperations,
): Promise<void> {
  const statusPath: string = path.join(directory, "exit-status");
  try {
    const completedAtMs: number = await operations.statMtime(statusPath);
    if (nowMs - completedAtMs < ARTIFACT_RETENTION_MILLISECONDS) {
      return;
    }
    const status: string = await operations.readFile(statusPath);
    if (EXIT_STATUS_PATTERN.test(status)) {
      await operations.removeDirectory(directory);
    }
  } catch {
    // Active, incomplete, or concurrently removed jobs are intentionally preserved.
  }
}

async function removeDirectoriesSequentially(
  entries: readonly string[],
  rootDirectory: string,
  nowMs: number,
  operations: ArtifactCleanupOperations,
  index = 0,
): Promise<void> {
  const entry: string | undefined = entries[index];
  if (entry === undefined) {
    return;
  }
  await removeExpiredDirectory(path.join(rootDirectory, entry), nowMs, operations);
  await removeDirectoriesSequentially(entries, rootDirectory, nowMs, operations, index + 1);
}

export async function removeExpiredArtifacts(options?: ArtifactCleanupOptions): Promise<void> {
  const operations: ArtifactCleanupOperations =
    options?.operations ?? systemArtifactCleanupOperations;
  const rootDirectory: string = options?.rootDirectory ?? tmpdir();
  const nowMs: number = (options?.nowMs ?? Date.now)();
  const entries: readonly string[] = await operations
    .readDirectory(rootDirectory)
    .catch(async (): Promise<readonly string[]> => []);
  const artifactDirectories: readonly string[] = entries.filter((entry: string): boolean =>
    ARTIFACT_DIRECTORY_PATTERN.test(entry),
  );
  await removeDirectoriesSequentially(artifactDirectories, rootDirectory, nowMs, operations);
}

export async function removeExpiredArtifactsIfDue(options?: ArtifactCleanupOptions): Promise<void> {
  const operations: ArtifactCleanupOperations =
    options?.operations ?? systemArtifactCleanupOperations;
  const rootDirectory: string = options?.rootDirectory ?? tmpdir();
  const nowMs: number = (options?.nowMs ?? Date.now)();
  const stampPath: string = path.join(rootDirectory, ARTIFACT_CLEANUP_STAMP_FILENAME);
  try {
    const previousCleanupMs: number = await operations.statMtime(stampPath);
    if (nowMs - previousCleanupMs < ARTIFACT_CLEANUP_INTERVAL_MILLISECONDS) {
      return;
    }
  } catch {
    // A missing stamp means cleanup has never run in this temporary directory.
  }
  await removeExpiredArtifacts({ ...options, nowMs: (): number => nowMs });
  await operations
    .writeFile(stampPath, `${new Date(nowMs).toISOString()}\n`)
    .catch((): void => undefined);
}

export class ArtifactCleaner {
  #cleanup: Promise<void>;
  readonly #options: ArtifactCleanupOptions;
  readonly #scheduler: CleanupScheduler;
  readonly #timer: CleanupTimer;

  constructor(options?: ArtifactCleanerOptions) {
    this.#options = options ?? {};
    this.#scheduler = options?.scheduler ?? SYSTEM_SCHEDULER;
    this.#cleanup = removeExpiredArtifactsIfDue(this.#options);
    this.#timer = this.#scheduler.schedule((): void => {
      this.#cleanup = this.#cleanup.then(async (): Promise<void> =>
        removeExpiredArtifactsIfDue(this.#options),
      );
    }, ARTIFACT_CLEANUP_INTERVAL_MILLISECONDS);
    this.#timer.unref();
  }

  stop(): void {
    this.#scheduler.clear(this.#timer);
  }
}
