#!/usr/bin/env bun
// This TypeScript file is executed with Bun.
import fs from "node:fs";
import process from "node:process";
import {
  type ClaudexBashInput,
  formatLocalTimestamp,
  shouldBackgroundClaudexBash,
} from "./src/policy.ts";

const MAX_TASK_SUMMARY_CHARACTERS = 160;

interface HookPayload {
  readonly hook_event_name?: unknown;
  readonly tool_input?: unknown;
  readonly tool_name?: unknown;
}

interface HookSpecificOutput {
  readonly hookEventName: "PreToolUse";
  readonly updatedInput: Record<string, unknown>;
}

export interface HookOutput {
  readonly hookSpecificOutput: HookSpecificOutput;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function claudexBashInput(value: Record<string, unknown>): ClaudexBashInput | undefined {
  const command: unknown = value.command;
  const timeout: unknown = value.timeout;
  const runInBackground: unknown = value.run_in_background;
  if (typeof command !== "string") {
    return undefined;
  }
  if (timeout !== undefined && typeof timeout !== "number") {
    return undefined;
  }
  if (runInBackground !== undefined && typeof runInBackground !== "boolean") {
    return undefined;
  }
  return {
    command,
    ...(timeout === undefined ? {} : { timeout }),
    ...(runInBackground === undefined ? {} : { run_in_background: runInBackground }),
  };
}

function submittedDescription(input: Record<string, unknown>, submittedAt: Date): string {
  const description: unknown = input.description;
  const command: unknown = input.command;
  const timestamp: string = formatLocalTimestamp(submittedAt, "submitted");
  if (typeof description === "string" && description.trim() !== "") {
    return `${timestamp} | ${description.trim().slice(0, MAX_TASK_SUMMARY_CHARACTERS)}`;
  }
  const summary: string = typeof command === "string" ? command : "Bash";
  return `${timestamp} | ${summary.slice(0, MAX_TASK_SUMMARY_CHARACTERS)}`;
}

export function claudexHookOutput(value: unknown, submittedAt: Date): HookOutput | undefined {
  if (!isRecord(value)) {
    return undefined;
  }
  const payload: HookPayload = value;
  if (payload.hook_event_name !== "PreToolUse" || payload.tool_name !== "Bash") {
    return undefined;
  }
  if (!isRecord(payload.tool_input)) {
    return undefined;
  }
  const input: ClaudexBashInput | undefined = claudexBashInput(payload.tool_input);
  if (input === undefined || !shouldBackgroundClaudexBash(input)) {
    return undefined;
  }
  return {
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      updatedInput: {
        ...payload.tool_input,
        description: submittedDescription(payload.tool_input, submittedAt),
        run_in_background: true,
      },
    },
  };
}

function run(): void {
  try {
    const payload: unknown = JSON.parse(fs.readFileSync(0, "utf8"));
    const output: HookOutput | undefined = claudexHookOutput(payload, new Date());
    if (output !== undefined) {
      process.stdout.write(`${JSON.stringify(output)}\n`);
    }
  } catch {
    // Invalid hook input is a no-op so Claude Code can continue normally.
  }
}

if (process.argv[1] === import.meta.filename) {
  run();
}
