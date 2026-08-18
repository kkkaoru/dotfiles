// This file runs with Bun.
import process from "node:process";
import type {
  ContentBlock,
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

interface ToolCallSnapshot {
  callId: string;
  content: ToolCallContent[];
  kind: string | undefined;
  metadata: Record<string, unknown>;
  rawInput: unknown;
  rawOutput: unknown;
  status: string | undefined;
  title: string;
}

interface ToolCallState {
  call: ToolCallSnapshot;
  completionRendered: boolean;
  startRendered: boolean;
}

interface HandleUpdateInput {
  output: DevinOutput;
  toolCalls: Map<string, ToolCallState>;
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

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasProperty(value: object, property: string): boolean {
  return Object.prototype.hasOwnProperty.call(value, property);
}

function stringValue(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function numberValue(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function formatUnknown(value: unknown): string {
  if (typeof value === "string") return value;
  const formatted = JSON.stringify(value, undefined, 2);
  return formatted ?? String(value);
}

function patchedString(
  previous: string | undefined,
  value: string | null | undefined,
  source: object,
  property: string,
): string | undefined {
  return hasProperty(source, property) ? (value ?? previous) : previous;
}

function patchedUnknown(
  previous: unknown,
  value: unknown,
  source: object,
  property: string,
): unknown {
  return hasProperty(source, property) ? value : previous;
}

function patchedContent(previous: ToolCallContent[], update: AcpToolCall | ToolCallUpdate) {
  if (!hasProperty(update, "content")) return previous;
  return update.content === null || update.content === undefined ? [] : update.content;
}

function mergeMetadata(
  previous: Record<string, unknown> | undefined,
  update: AcpToolCall["_meta"] | ToolCallUpdate["_meta"],
): Record<string, unknown> {
  if (update === undefined) return previous ?? {};
  if (!isRecord(update)) return {};
  return { ...previous, ...update };
}

function mergeToolCallState(
  existing: ToolCallState | undefined,
  update: AcpToolCall | ToolCallUpdate,
): ToolCallState {
  const previous = existing?.call;
  const call: ToolCallSnapshot = {
    callId: update.toolCallId,
    content: patchedContent(previous?.content ?? [], update),
    kind: patchedString(previous?.kind, update.kind, update, "kind"),
    metadata: mergeMetadata(previous?.metadata, update._meta),
    rawInput: patchedUnknown(previous?.rawInput, update.rawInput, update, "rawInput"),
    rawOutput: patchedUnknown(previous?.rawOutput, update.rawOutput, update, "rawOutput"),
    status: patchedString(previous?.status, update.status, update, "status"),
    title:
      patchedString(previous?.title, update.title, update, "title") ??
      patchedString(previous?.kind, update.kind, update, "kind") ??
      update.toolCallId,
  };
  return {
    call,
    completionRendered: existing?.completionRendered ?? false,
    startRendered: existing?.startRendered ?? false,
  };
}

function formatContentBlock(content: ContentBlock): string | undefined {
  if (content.type === "text") return content.text;
  if (content.type === "resource") {
    return "text" in content.resource
      ? content.resource.text
      : `[binary resource: ${content.resource.mimeType ?? "unknown"}]`;
  }
  if (content.type === "resource_link") return `Resource: ${content.name} (${content.uri})`;
  if (content.type === "image") return `[image: ${content.mimeType}]`;
  if (content.type === "audio") return `[audio: ${content.mimeType}]`;
  return undefined;
}

function formatToolCallContent(item: ToolCallContent): string | undefined {
  if (item.type === "diff") return formatDiff(item);
  if (item.type === "terminal") return `Terminal ID: ${item.terminalId}`;
  if (item.type === "content") return formatContentBlock(item.content);
  return undefined;
}

function commandValue(call: ToolCallSnapshot): string | undefined {
  const input = isRecord(call.rawInput) ? call.rawInput : undefined;
  const command = stringValue(input?.["command"]);
  if (command !== undefined) return command;
  const preview = call.content.find(
    (item) =>
      item.type === "content" &&
      item.content.type === "resource" &&
      isRecord(item.content._meta) &&
      item.content._meta["cognition.ai/preview_is_shell_command"] === true,
  );
  if (
    preview?.type === "content" &&
    preview.content.type === "resource" &&
    "text" in preview.content.resource
  ) {
    return preview.content.resource.text;
  }
  return undefined;
}

function pathValue(call: ToolCallSnapshot): string | undefined {
  const input = isRecord(call.rawInput) ? call.rawInput : undefined;
  return stringValue(input?.["file_path"]) ?? stringValue(input?.["path"]);
}

function workingDirectoryValue(call: ToolCallSnapshot): string | undefined {
  const input = isRecord(call.rawInput) ? call.rawInput : undefined;
  const metadataCwd = call.metadata["cognition.ai/cwd"];
  return stringValue(input?.["workdir"]) ?? stringValue(input?.["cwd"]) ?? stringValue(metadataCwd);
}

function formatToolCallInput(call: ToolCallSnapshot): string | undefined {
  const sections: string[] = [];
  const command = commandValue(call);
  if (command !== undefined) sections.push(`**Command:**\n\`\`\`sh\n${command}\n\`\`\``);
  const path = pathValue(call);
  if (path !== undefined) sections.push(`**Path:** \`${path}\``);
  const workingDirectory = workingDirectoryValue(call);
  if (workingDirectory !== undefined) {
    sections.push(`**Working directory:** \`${workingDirectory}\``);
  }
  if (sections.length === 0 && call.rawInput !== undefined && call.rawInput !== null) {
    sections.push(`**Input:**\n\`\`\`json\n${formatUnknown(call.rawInput)}\n\`\`\``);
  }
  return sections.length > 0 ? sections.join("\n\n") : undefined;
}

function formatToolCallBody(call: ToolCallSnapshot): string | undefined {
  const body = call.content
    .map(formatToolCallContent)
    .filter((text): text is string => text !== undefined && text.length > 0)
    .join("\n");
  return body.length > 0 ? body : undefined;
}

function formatRawOutput(rawOutput: unknown, contentBody: string | undefined): string | undefined {
  if (rawOutput === undefined || rawOutput === null) return undefined;
  if (isRecord(rawOutput)) {
    const sections: string[] = [];
    const stdout = stringValue(rawOutput["stdout"]);
    const stderr = stringValue(rawOutput["stderr"]);
    const output = stringValue(rawOutput["output"]);
    if (stdout !== undefined) sections.push(`**stdout:**\n\`\`\`\n${stdout}\n\`\`\``);
    if (stderr !== undefined) sections.push(`**stderr:**\n\`\`\`\n${stderr}\n\`\`\``);
    if (output !== undefined && output !== contentBody) {
      sections.push(`**Output:**\n\`\`\`\n${output}\n\`\`\``);
    }
    if (sections.length > 0) return sections.join("\n\n");
  }
  if (typeof rawOutput === "string" && rawOutput === contentBody) return undefined;
  return `**Raw output:**\n\`\`\`json\n${formatUnknown(rawOutput)}\n\`\`\``;
}

interface TerminalExit {
  exitCode: number | undefined;
  signal: string | undefined;
  terminalId: string | undefined;
}

function terminalExit(call: ToolCallSnapshot): TerminalExit | undefined {
  const metadataExit = isRecord(call.metadata["terminal_exit"])
    ? call.metadata["terminal_exit"]
    : undefined;
  const rawOutput = isRecord(call.rawOutput) ? call.rawOutput : undefined;
  const exitCode =
    numberValue(metadataExit?.["exit_code"]) ??
    numberValue(metadataExit?.["exitCode"]) ??
    numberValue(rawOutput?.["exit_code"]) ??
    numberValue(rawOutput?.["exitCode"]);
  const signal = stringValue(metadataExit?.["signal"]) ?? stringValue(rawOutput?.["signal"]);
  const terminalId =
    stringValue(metadataExit?.["terminal_id"]) ?? stringValue(metadataExit?.["terminalId"]);
  return exitCode === undefined && signal === undefined && terminalId === undefined
    ? undefined
    : { exitCode, signal, terminalId };
}

function formatTerminalExit(call: ToolCallSnapshot): string | undefined {
  const exit = terminalExit(call);
  if (!exit) return undefined;
  const sections: string[] = [];
  if (exit.exitCode !== undefined) sections.push(`**Exit code:** ${exit.exitCode}`);
  if (exit.signal !== undefined) sections.push(`**Signal:** ${exit.signal}`);
  if (exit.terminalId !== undefined) sections.push(`**Terminal:** \`${exit.terminalId}\``);
  return sections.join("\n");
}

function toolStatusSymbol(status: string | undefined): string {
  if (status === "completed") return "✓";
  if (status === "failed" || status === "cancelled") return "✗";
  return "▶";
}

function toolHeader(call: ToolCallSnapshot, status: string): string {
  const kind = call.kind ?? "other";
  return `${toolStatusSymbol(call.status)} **${call.title}** (${kind}) — ${status}`;
}

function formatToolCallStart(call: ToolCallSnapshot): string {
  return [toolHeader(call, call.status ?? "started"), formatToolCallInput(call)]
    .filter((text): text is string => text !== undefined)
    .join("\n\n");
}

function formatToolCallCompletion(call: ToolCallSnapshot): string {
  const body = formatToolCallBody(call);
  const rawOutput = formatRawOutput(call.rawOutput, body);
  const outputLabel =
    call.kind === "execute" ? "**Terminal output (stdout/stderr):**" : "**Tool output:**";
  const output = body === undefined ? undefined : `${outputLabel}\n${body}`;
  return [
    toolHeader(call, call.status ?? "unknown"),
    formatToolCallInput(call),
    output,
    rawOutput,
    formatTerminalExit(call),
  ]
    .filter((text): text is string => text !== undefined)
    .join("\n\n");
}

function isTerminalStatus(status: string | undefined): boolean {
  return status === "completed" || status === "failed" || status === "cancelled";
}

function renderToolCall(
  output: DevinOutput,
  toolCalls: Map<string, ToolCallState>,
  update: AcpToolCall | ToolCallUpdate,
): void {
  const state = mergeToolCallState(toolCalls.get(update.toolCallId), update);
  toolCalls.set(update.toolCallId, state);
  if (!state.startRendered) {
    output.appendTextBlock(formatToolCallStart(state.call));
    state.startRendered = true;
  }
  if (isTerminalStatus(state.call.status) && !state.completionRendered) {
    output.appendTextBlock(formatToolCallCompletion(state.call));
    state.completionRendered = true;
  }
}

function handleUpdate({ update, output, toolCalls }: HandleUpdateInput): void {
  if (update.sessionUpdate === "agent_message_chunk" && update.content.type === "text") {
    output.appendText(update.content.text);
  }
  if (update.sessionUpdate === "agent_thought_chunk" && update.content.type === "text") {
    output.appendThinking(update.content.text);
  }
  if (update.sessionUpdate === "tool_call" || update.sessionUpdate === "tool_call_update") {
    renderToolCall(output, toolCalls, update);
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
    const toolCalls = new Map<string, ToolCallState>();
    await runDevinJob({
      continuationPrompt: buildContinuationPrompt(request.context),
      cwd: process.cwd(),
      initialPrompt: buildDevinTranscript(request.context),
      modelId: request.model.id,
      sessionId: request.sessionId,
      signal: request.signal,
      onUpdate: (update) => handleUpdate({ update, output: request.output, toolCalls }),
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
