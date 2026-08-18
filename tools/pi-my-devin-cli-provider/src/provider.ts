// This file runs with Bun.
import process from "node:process";
import type {
  Diff as AcpDiff,
  SessionUpdate,
  ToolCall as AcpToolCall,
  ToolCallContent,
  ToolCallUpdate,
} from "@agentclientprotocol/sdk";
import type {
  Api,
  AssistantMessageEventStream,
  Context,
  Model,
  SimpleStreamOptions,
} from "@earendil-works/pi-ai";
import { buildContinuationPrompt, buildDevinTranscript } from "./context.ts";
import { createDevinOutput, type DevinOutput } from "./stream-output.ts";
import { resolveDevinSessionId, runDevinJob } from "./runtime.ts";

interface ActiveState {
  count: number;
}

interface RunRequest {
  context: Context;
  model: Model<Api>;
  output: DevinOutput;
  sessionId: string;
  signal: AbortSignal | undefined;
}

interface HandleUpdateInput {
  output: DevinOutput;
  renderedToolCallIds: Set<string>;
  toolCalls: Map<string, AcpToolCall | ToolCallUpdate>;
  update: SessionUpdate;
}

const activeState: ActiveState = { count: 0 };

function prefixEachLine(text: string, prefix: string): string {
  if (text.length === 0) return "";
  const lines = text.split("\n");
  const last = lines[lines.length - 1];
  const trimmed = last?.length === 0 ? lines.slice(0, -1) : lines;
  return trimmed.map((line) => `${prefix}${line}`).join("\n");
}

function formatDiff(diff: AcpDiff): string {
  const path = diff.path;
  const oldText = diff.oldText;
  const newText = diff.newText;
  if (oldText === undefined || oldText === null) {
    return `\`\`\`diff\n+++ ${path}\n${prefixEachLine(newText, "+")}\n\`\`\``;
  }
  const newLines = newText.length > 0 ? `\n${prefixEachLine(newText, "+")}` : "";
  return `\`\`\`diff\n--- ${path}\n+++ ${path}\n${prefixEachLine(oldText, "-")}${newLines}\n\`\`\``;
}

function formatToolCallContent(item: ToolCallContent): string | undefined {
  if (item.type === "diff") return formatDiff(item);
  if (item.type === "content" && item.content.type === "text") return item.content.text;
  return undefined;
}

function formatToolCallUpdate(update: AcpToolCall | ToolCallUpdate): string | undefined {
  const status = update.status;
  if (status === "pending" || status === "in_progress") return undefined;
  const body = (update.content ?? [])
    .map(formatToolCallContent)
    .filter((text): text is string => text !== undefined)
    .join("\n");
  if (body.length === 0) return undefined;
  const title = update.title ?? "";
  const kind = update.kind ?? "other";
  const header = title.length > 0 ? `**${title}** (${kind})\n\n` : `**${kind}**\n\n`;
  return `${header}${body}\n\n`;
}

function handleUpdate({ update, output, renderedToolCallIds, toolCalls }: HandleUpdateInput): void {
  if (update.sessionUpdate === "agent_message_chunk" && update.content.type === "text") {
    output.appendText(update.content.text);
  }
  if (update.sessionUpdate === "agent_thought_chunk" && update.content.type === "text") {
    output.appendThinking(update.content.text);
  }
  if (update.sessionUpdate === "tool_call" || update.sessionUpdate === "tool_call_update") {
    const existing = toolCalls.get(update.toolCallId);
    const merged = { ...existing, ...update } as AcpToolCall | ToolCallUpdate;
    toolCalls.set(update.toolCallId, merged);
    if (
      !renderedToolCallIds.has(update.toolCallId) &&
      (merged.status === "completed" || merged.status === "failed")
    ) {
      const text = formatToolCallUpdate(merged);
      if (text !== undefined) {
        renderedToolCallIds.add(update.toolCallId);
        output.appendTextBlock(text);
      }
    }
  }
}

async function waitUntilIdle(): Promise<void> {
  if (activeState.count === 0) return;
  await new Promise<void>((resolve) => setImmediate(resolve));
  return waitUntilIdle();
}

async function runAcpRequest(request: RunRequest): Promise<void> {
  activeState.count += 1;
  try {
    const renderedToolCallIds = new Set<string>();
    const toolCalls = new Map<string, AcpToolCall | ToolCallUpdate>();
    await runDevinJob({
      continuationPrompt: buildContinuationPrompt(request.context),
      cwd: process.cwd(),
      initialPrompt: buildDevinTranscript(request.context),
      modelId: request.model.id,
      sessionId: request.sessionId,
      signal: request.signal,
      onUpdate: (update) =>
        handleUpdate({ update, output: request.output, renderedToolCallIds, toolCalls }),
    });
    request.output.finish();
  } finally {
    activeState.count -= 1;
  }
}

export { createDevinSessionId, resolveDevinSessionId, selectPermission } from "./runtime.ts";

export function streamDevin(
  ...parameters: [model: Model<Api>, context: Context, options?: SimpleStreamOptions]
): AssistantMessageEventStream {
  const [model, context, options] = parameters;
  const output: DevinOutput = createDevinOutput(model);
  const sessionId: string = resolveDevinSessionId(options?.sessionId);
  void runAcpRequest({ model, context, output, sessionId, signal: options?.signal }).catch(
    (error: unknown) => {
      output.fail(error, options?.signal?.aborted === true);
    },
  );
  return output.stream;
}

export const devinProviderTestApi = {
  activeCount: (): number => activeState.count,
  waitForIdle: (): Promise<void> => waitUntilIdle(),
};
