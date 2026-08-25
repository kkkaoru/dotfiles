// This TypeScript file is executed with Bun.
import type { ToolCall } from "@earendil-works/pi-ai";
import {
  type ClaudexBashInput,
  formatLocalTimestamp,
  shouldBackgroundClaudexBash,
} from "@kkkaoru/pi-tmux-timeout-extension/policy";
import { isRecord } from "./protocol.ts";

const MAX_TASK_SUMMARY_CHARACTERS = 160;

function claudexBashInput(value: unknown): ClaudexBashInput | undefined {
  if (!isRecord(value) || typeof value["command"] !== "string") {
    return undefined;
  }
  const timeout: unknown = value["timeout"];
  const runInBackground: unknown = value["run_in_background"];
  if (timeout !== undefined && typeof timeout !== "number") {
    return undefined;
  }
  if (runInBackground !== undefined && typeof runInBackground !== "boolean") {
    return undefined;
  }
  return {
    command: value["command"],
    ...(timeout === undefined ? {} : { timeout }),
    ...(runInBackground === undefined ? {} : { run_in_background: runInBackground }),
  };
}

function taskSummary(argumentsValue: Record<string, unknown>): string {
  const description: unknown = argumentsValue["description"];
  const command: unknown = argumentsValue["command"];
  if (typeof description === "string" && description.trim() !== "") {
    return description.trim().slice(0, MAX_TASK_SUMMARY_CHARACTERS);
  }
  const summary: string = typeof command === "string" ? command : "Bash";
  return summary.slice(0, MAX_TASK_SUMMARY_CHARACTERS);
}

export function backgroundLongClaudexBash(toolCall: ToolCall, submittedAt: Date): ToolCall {
  const input: ClaudexBashInput | undefined = claudexBashInput(toolCall.arguments);
  if (toolCall.name !== "Bash" || input === undefined || !shouldBackgroundClaudexBash(input)) {
    return toolCall;
  }
  const argumentsValue: Record<string, unknown> = toolCall.arguments;
  return {
    ...toolCall,
    arguments: {
      ...argumentsValue,
      description: `${formatLocalTimestamp(submittedAt, "submitted")} | ${taskSummary(argumentsValue)}`,
      run_in_background: true,
    },
  };
}
