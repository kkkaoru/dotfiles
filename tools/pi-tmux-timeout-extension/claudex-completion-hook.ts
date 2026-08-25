#!/usr/bin/env bun
// This TypeScript file is executed with Bun.
import fs from "node:fs";
import fsPromises from "node:fs/promises";
import process from "node:process";
import { formatLocalTimestamp } from "./src/policy.ts";

export interface BackgroundTask {
  readonly command: string;
  readonly outputPath: string;
  readonly taskId: string;
}

export interface BackgroundTaskCompletion extends BackgroundTask {
  readonly exitCode: number;
}

export interface CompletionWatcher {
  readonly close: () => void;
  readonly onError: (listener: (error: Error) => void) => void;
}

export interface CompletionOperations {
  readonly readFile: (filePath: string) => Promise<string>;
  readonly watch: (filePath: string, onChange: () => void) => CompletionWatcher;
}

interface CompletionWatchState {
  watcher?: CompletionWatcher;
}

const BACKGROUND_RESULT_PATTERN =
  /Command running in background with ID:\s*([A-Za-z0-9_-]+).*?Output is being written to:\s*([^\s]+?\.output)/su;
const EXIT_STATUS_PATTERN = /\[exited with code (-?\d+)\]\s*$/u;
const SYSTEM_OPERATIONS: CompletionOperations = {
  readFile: async (filePath: string): Promise<string> => fsPromises.readFile(filePath, "utf8"),
  watch: (filePath: string, onChange: () => void): CompletionWatcher => {
    const watcher = fs.watch(filePath, onChange);
    return {
      close: (): void => watcher.close(),
      onError: (listener: (error: Error) => void): void => {
        watcher.on("error", listener);
      },
    };
  },
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function textValues(value: unknown): readonly string[] {
  if (typeof value === "string") {
    return [value];
  }
  if (Array.isArray(value)) {
    return value.flatMap((entry: unknown): readonly string[] => textValues(entry));
  }
  return isRecord(value)
    ? Object.values(value).flatMap((entry: unknown): readonly string[] => textValues(entry))
    : [];
}

function toolCommand(payload: Record<string, unknown>): string {
  const input: unknown = payload.tool_input;
  return isRecord(input) && typeof input.command === "string" ? input.command : "Bash";
}

export function backgroundTaskFromHook(value: unknown): BackgroundTask | undefined {
  if (!isRecord(value) || value.hook_event_name !== "PostToolUse" || value.tool_name !== "Bash") {
    return undefined;
  }
  const resultText: string = textValues(value.tool_response).join("\n");
  const match: RegExpExecArray | null = BACKGROUND_RESULT_PATTERN.exec(resultText);
  return match?.[1] === undefined || match[2] === undefined
    ? undefined
    : { command: toolCommand(value), outputPath: match[2], taskId: match[1] };
}

function exitCodeFromOutput(output: string): number | undefined {
  const match: RegExpExecArray | null = EXIT_STATUS_PATTERN.exec(output);
  return match?.[1] === undefined ? undefined : Number(match[1]);
}

export async function waitForBackgroundTask(
  task: BackgroundTask,
  operations: CompletionOperations = SYSTEM_OPERATIONS,
): Promise<BackgroundTaskCompletion> {
  return new Promise<BackgroundTaskCompletion>((resolve, reject): void => {
    const state: CompletionWatchState = {};
    const checkCompletion = async (): Promise<void> => {
      const output: string = await operations.readFile(task.outputPath);
      const exitCode: number | undefined = exitCodeFromOutput(output);
      if (exitCode !== undefined) {
        state.watcher?.close();
        resolve({ ...task, exitCode });
      }
    };
    state.watcher = operations.watch(task.outputPath, (): void => {
      checkCompletion().catch(reject);
    });
    state.watcher.onError(reject);
    checkCompletion().catch(reject);
  });
}

export function completionMessage(completion: BackgroundTaskCompletion, completedAt: Date): string {
  const failure: string =
    completion.exitCode === 0 ? "" : ` | failed=${String(completion.exitCode)}`;
  return `${formatLocalTimestamp(completedAt, "submitted")} | task=${completion.taskId}${failure} | ${completion.command}\n${completion.outputPath}\nInspect this exact output in the same SubAgent context. Do not TaskStop, terminate, or launch another Agent.`;
}

async function run(): Promise<void> {
  try {
    const payload: unknown = JSON.parse(fs.readFileSync(0, "utf8"));
    const task: BackgroundTask | undefined = backgroundTaskFromHook(payload);
    if (task === undefined) {
      return;
    }
    const completion: BackgroundTaskCompletion = await waitForBackgroundTask(task);
    process.stderr.write(`${completionMessage(completion, new Date())}\n`);
    process.exitCode = 2;
  } catch {
    // Invalid or unrelated hook input is a no-op so Claude Code can continue normally.
  }
}

if (process.argv[1] === import.meta.filename) {
  await run();
}
