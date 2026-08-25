// This TypeScript file is executed with Bun.
import fs from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import process from "node:process";
import type { TmuxLaunch } from "./tmux.ts";

export const DELIVERY_MARKER_FILENAME = "completion-delivered";
export const LAUNCH_METADATA_FILENAME = "launch.json";
export const TMUX_SESSION_ENTRY_TYPE = "pi-tmux-launch-v1";

interface PersistedTmuxLaunch {
  readonly completionChannel: string;
  readonly logPath: string;
  readonly sessionName: string;
  readonly statusPath: string;
  readonly submittedAt: string;
  readonly taskCommand: string;
}

export interface PersistenceOperations {
  readonly exists: (filePath: string) => boolean;
  readonly readDirectory: (directory: string) => readonly string[];
  readonly readFile: (filePath: string) => string;
  readonly statBirthtime: (filePath: string) => number;
  readonly writeFile: (filePath: string, content: string) => void;
}

export interface RecoveryOptions {
  readonly operations?: PersistenceOperations;
  readonly pid?: number;
  readonly rootDirectory?: string;
}

const SYSTEM_OPERATIONS: PersistenceOperations = {
  exists: fs.existsSync,
  readDirectory: fs.readdirSync,
  readFile: (filePath: string): string => fs.readFileSync(filePath, "utf8"),
  statBirthtime: (filePath: string): number => fs.statSync(filePath).birthtimeMs,
  writeFile: (filePath: string, content: string): void => fs.writeFileSync(filePath, content),
};

function persistedLaunch(launch: TmuxLaunch): PersistedTmuxLaunch {
  return {
    completionChannel: launch.completionChannel,
    logPath: launch.logPath,
    sessionName: launch.sessionName,
    statusPath: launch.statusPath,
    submittedAt: launch.submittedAt,
    taskCommand: launch.taskCommand,
  };
}

export function serializeLaunchMetadata(launch: TmuxLaunch): string {
  return JSON.stringify(persistedLaunch(launch));
}

export function persistTmuxLaunch(
  writer: ((customType: string, data: unknown) => void) | undefined,
  launch: TmuxLaunch,
): void {
  writer?.(TMUX_SESSION_ENTRY_TYPE, persistedLaunch(launch));
}

export function deliveryMarkerPath(launch: TmuxLaunch): string {
  return path.join(path.dirname(launch.statusPath), DELIVERY_MARKER_FILENAME);
}

export function markCompletionDelivered(
  launch: TmuxLaunch,
  operations: PersistenceOperations = SYSTEM_OPERATIONS,
): void {
  operations.writeFile(deliveryMarkerPath(launch), `${new Date().toISOString()}\n`);
}

function isStringProperty(value: object, key: keyof PersistedTmuxLaunch): boolean {
  return key in value && typeof (value as Record<string, unknown>)[key] === "string";
}

const METADATA_KEYS = [
  "completionChannel",
  "logPath",
  "sessionName",
  "statusPath",
  "submittedAt",
  "taskCommand",
] as const satisfies readonly (keyof PersistedTmuxLaunch)[];

function parseMetadata(
  content: string,
  directory: string,
  sessionName: string,
): TmuxLaunch | undefined {
  try {
    const value: unknown = JSON.parse(content);
    if (
      typeof value !== "object" ||
      value === null ||
      !METADATA_KEYS.every((key): boolean => isStringProperty(value, key))
    ) {
      return undefined;
    }
    const metadata = value as PersistedTmuxLaunch;
    if (
      metadata.sessionName !== sessionName ||
      metadata.completionChannel !== `${sessionName}-complete` ||
      metadata.logPath !== path.join(directory, "output.log") ||
      metadata.statusPath !== path.join(directory, "exit-status") ||
      !Number.isFinite(Date.parse(metadata.submittedAt))
    ) {
      return undefined;
    }
    return { command: "", ...metadata };
  } catch {
    return undefined;
  }
}

function legacyLaunch(
  directory: string,
  sessionName: string,
  operations: PersistenceOperations,
): TmuxLaunch | undefined {
  const logPath: string = path.join(directory, "output.log");
  if (!operations.exists(logPath)) {
    return undefined;
  }
  return {
    command: "",
    completionChannel: `${sessionName}-complete`,
    logPath,
    sessionName,
    statusPath: path.join(directory, "exit-status"),
    submittedAt: new Date(operations.statBirthtime(directory)).toISOString(),
    taskCommand: `recovered tmux job ${sessionName}`,
  };
}

function readDirectory(directory: string, operations: PersistenceOperations): readonly string[] {
  try {
    return operations.readDirectory(directory);
  } catch {
    return [];
  }
}

function recoveryInput(options?: RecoveryOptions): {
  readonly entries: readonly string[];
  readonly operations: PersistenceOperations;
  readonly pattern: RegExp;
  readonly rootDirectory: string;
} {
  const operations: PersistenceOperations = options?.operations ?? SYSTEM_OPERATIONS;
  const pid: number = options?.pid ?? process.pid;
  const rootDirectory: string = options?.rootDirectory ?? tmpdir();
  return {
    entries: readDirectory(rootDirectory, operations),
    operations,
    pattern: new RegExp(`^pi-tmux-${String(pid)}-(\\d+)$`, "u"),
    rootDirectory,
  };
}

function launchFromSessionEntry(entry: unknown): TmuxLaunch | undefined {
  try {
    const candidate = entry as {
      readonly customType?: unknown;
      readonly data?: Partial<PersistedTmuxLaunch>;
      readonly type?: unknown;
    };
    if (candidate.type !== "custom" || candidate.customType !== TMUX_SESSION_ENTRY_TYPE) {
      return undefined;
    }
    const data: Partial<PersistedTmuxLaunch> = candidate.data ?? {};
    return parseMetadata(
      JSON.stringify(data),
      path.dirname(data.statusPath ?? ""),
      data.sessionName ?? "",
    );
  } catch {
    return undefined;
  }
}

export function recoverSessionTmuxLaunches(
  entries: readonly unknown[],
  operations: PersistenceOperations = SYSTEM_OPERATIONS,
): readonly TmuxLaunch[] {
  const launches = new Map<string, TmuxLaunch>();
  const recovered: readonly TmuxLaunch[] = entries.flatMap(
    (entry: unknown): readonly TmuxLaunch[] => {
      const launch: TmuxLaunch | undefined = launchFromSessionEntry(entry);
      return launch === undefined ? [] : [launch];
    },
  );
  for (const launch of recovered) {
    if (!operations.exists(deliveryMarkerPath(launch))) {
      launches.set(launch.sessionName, launch);
    }
  }
  return [...launches.values()];
}

export function nextTmuxLaunchId(options?: RecoveryOptions): number {
  const { entries, pattern } = recoveryInput(options);
  let nextId = 1;
  for (const sessionName of entries) {
    const match: RegExpExecArray | null = pattern.exec(sessionName);
    if (match?.[1] !== undefined) {
      nextId = Math.max(nextId, Number(match[1]) + 1);
    }
  }
  return nextId;
}

function recoverEntry(
  sessionName: string,
  rootDirectory: string,
  pattern: RegExp,
  operations: PersistenceOperations,
): TmuxLaunch | undefined {
  if (!pattern.test(sessionName)) {
    return undefined;
  }
  try {
    const directory: string = path.join(rootDirectory, sessionName);
    if (operations.exists(path.join(directory, DELIVERY_MARKER_FILENAME))) {
      return undefined;
    }
    const metadataPath: string = path.join(directory, LAUNCH_METADATA_FILENAME);
    return operations.exists(metadataPath)
      ? parseMetadata(operations.readFile(metadataPath), directory, sessionName)
      : legacyLaunch(directory, sessionName, operations);
  } catch {
    return undefined;
  }
}

export function recoverTmuxLaunches(options?: RecoveryOptions): readonly TmuxLaunch[] {
  const { entries, operations, pattern, rootDirectory } = recoveryInput(options);
  return entries.flatMap((sessionName: string): readonly TmuxLaunch[] => {
    const launch: TmuxLaunch | undefined = recoverEntry(
      sessionName,
      rootDirectory,
      pattern,
      operations,
    );
    return launch === undefined ? [] : [launch];
  });
}
