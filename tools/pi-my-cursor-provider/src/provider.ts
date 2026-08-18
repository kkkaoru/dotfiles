// This file runs with Bun.
import { randomUUID } from "node:crypto";
import {
  Agent,
  type AgentOptions,
  type InteractionUpdate,
  type Run,
  type SDKAgent,
  type SDKCustomTool,
  type SDKJsonValue,
  type ShellOutputDeltaUpdate,
  type TokenUsage,
  type ToolCall,
  type ToolCallDeltaUpdate,
  type ToolName,
  type TurnEndedUpdate,
} from "@cursor/sdk";
import type {
  Api,
  AssistantMessageEventStream,
  Context,
  Model,
  SimpleStreamOptions,
  Tool,
  ToolResultMessage,
} from "@earendil-works/pi-ai";
import { buildCursorMessage, findToolResults, toolResultToSdk, toSdkJsonValue } from "./context.ts";
import { cursorModelSelection } from "./models.ts";
import { ensureCursorPlatform } from "./platform.ts";
import { cursorProcessAgentId, getProcessCursorStore } from "./store.ts";
import { createCursorOutput, type CursorOutput } from "./stream-output.ts";

interface PendingInvocation {
  readonly id: string;
  readonly session: CursorSession;
  resolve(result: SDKJsonValue): void;
  reject(error: Error): void;
}

const pendingByToolCallId = new Map<string, PendingInvocation>();
const TOOL_BATCH_DELAY_MS = 0;
const DEFAULT_CLAIM_TIMEOUT_MS = 1_000; // Enough for the previous local run to release because the process-scoped store isolates agents, and a longer timeout hurts multi-turn latency.
const CURSOR_ALLOWED_TOOLS: readonly ToolName[] = ["mcp", "webSearch", "semSearch", "shell"];

interface IndependentSessionState {
  claimTimeoutMs: number;
  active: CursorSession | undefined;
  claimQueue: Promise<void>;
}

const independentSessionState: IndependentSessionState = {
  claimTimeoutMs: DEFAULT_CLAIM_TIMEOUT_MS,
  active: undefined,
  claimQueue: Promise.resolve(),
};

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

function schemaForTool(tool: Tool): Record<string, SDKJsonValue> {
  const schema = toSdkJsonValue(tool.parameters);
  return schema !== undefined &&
    !Array.isArray(schema) &&
    typeof schema === "object" &&
    schema !== null
    ? schema
    : { type: "object" };
}

function updateUsage(output: CursorOutput, usage: TokenUsage): void {
  output.partial.usage.input = usage.inputTokens;
  output.partial.usage.output = usage.outputTokens;
  output.partial.usage.cacheRead = usage.cacheReadTokens;
  output.partial.usage.cacheWrite = usage.cacheWriteTokens;
  output.partial.usage.totalTokens = usage.totalTokens;
}

function findPendingSession(toolResults: readonly ToolResultMessage[]): CursorSession | undefined {
  return toolResults
    .map((result) => pendingByToolCallId.get(result.toolCallId)?.session)
    .find((session) => session !== undefined);
}

const SHELL_OUTPUT_KEYS: readonly string[] = ["text", "data", "stdout", "stderr", "output"];

function shellOutputText(event: Record<string, unknown>): string | undefined {
  for (const key of SHELL_OUTPUT_KEYS) {
    const value = event[key];
    if (typeof value === "string") return value;
  }
  return undefined;
}

function formatDiff(path: string, diffString: string | undefined): string {
  const body = diffString ?? "";
  const hasHeader = /^(---|\+\+\+|diff )/.test(body);
  const header = hasHeader ? "" : `--- ${path}\n+++ ${path}\n`;
  return `\`\`\`diff\n${header}${body}\n\`\`\``;
}

function formatToolCallArgs(toolCall: ToolCall): string {
  return `\`\`\`json\n${JSON.stringify(toolCall.args, undefined, 2)}\n\`\`\``;
}

function formatToolCallResult(toolCall: ToolCall): string | undefined {
  if (toolCall.result === undefined) return undefined;
  if (toolCall.result.status === "error") return `Error: ${String(toolCall.result.error)}`;
  switch (toolCall.type) {
    case "edit": {
      const value = toolCall.result.value;
      return formatDiff(toolCall.args.path, value.diffString);
    }
    case "shell": {
      const value = toolCall.result.value;
      const parts = [
        `Exit code: ${value.exitCode}`,
        `**stdout:**\n\`\`\`\n${value.stdout}\n\`\`\``,
      ];
      if (value.stderr.length > 0) {
        parts.push(`**stderr:**\n\`\`\`\n${value.stderr}\n\`\`\``);
      }
      return parts.join("\n\n");
    }
    case "read": {
      const value = toolCall.result.value;
      return `\`\`\`\n${value.content}\n\`\`\`\n\n${value.totalLines} lines, ${value.fileSize} bytes`;
    }
    case "write": {
      const value = toolCall.result.value;
      const after =
        value.fileContentAfterWrite === undefined
          ? ""
          : `\n\nNew content:\n\`\`\`\n${value.fileContentAfterWrite}\n\`\`\``;
      return `Wrote ${value.linesCreated} lines (${value.fileSize} bytes)${after}`;
    }
    case "delete": {
      const value = toolCall.result.value;
      return `Deleted ${value.fileSize} bytes`;
    }
    case "glob": {
      const value = toolCall.result.value;
      const files = value.files.join("\n");
      const truncated = `${value.clientTruncated ? " (client truncated)" : ""}${
        value.ripgrepTruncated ? " (ripgrep truncated)" : ""
      }`;
      return `${files}\n\nTotal: ${value.totalFiles}${truncated}`;
    }
    case "mcp": {
      const value = toolCall.result.value;
      const text = value.content
        .map((item) => item.text?.text ?? "")
        .filter((line) => line.length > 0)
        .join("\n");
      return text.length > 0 ? text : `isError: ${value.isError}`;
    }
    default: {
      const value = toolCall.result.value;
      return `\`\`\`json\n${JSON.stringify(value, undefined, 2)}\n\`\`\``;
    }
  }
}

function formatCursorToolCallUpdate(toolCall: ToolCall, status: string): string | undefined {
  const result = status === "started" ? undefined : formatToolCallResult(toolCall);
  const header = `**${toolCall.type}** (${status})`;
  if (status === "started") return `${header}\n`;
  const args = formatToolCallArgs(toolCall);
  const sections = [args, result].filter((s): s is string => s !== undefined);
  if (sections.length === 0) return `${header}\n`;
  return `${header}\n\n${sections.join("\n\n")}\n`;
}

class CursorSession {
  private readonly context: Context;
  private readonly model: Model<Api>;
  private readonly options: SimpleStreamOptions | undefined;
  private readonly invocations = new Map<string, PendingInvocation>();
  private agent?: SDKAgent;
  private run?: Run;
  private output: CursorOutput | undefined;
  private abortCleanup: (() => void) | undefined;
  private currentSignal: AbortSignal | undefined;
  private toolBatchTimer: ReturnType<typeof setTimeout> | undefined;
  private disposed = false;
  private settle!: Promise<void>;
  private resolveSettle!: () => void;
  private completedToolCallIds = new Set<string>();
  private startedToolCallIds = new Set<string>();

  constructor(context: Context, model: Model<Api>, options: SimpleStreamOptions | undefined) {
    this.context = context;
    this.model = model;
    this.options = options;
    this.settle = new Promise((resolve) => {
      this.resolveSettle = resolve;
    });
  }

  get settled(): Promise<void> {
    return this.settle;
  }

  async shutdown(reason: Error): Promise<void> {
    await this.run?.cancel().catch(() => undefined);
    await this.fail(reason);
  }

  attach(output: CursorOutput, signal: AbortSignal | undefined): void {
    this.detachOutput();
    this.output = output;
    this.currentSignal = signal;
    if (!signal) return;
    const abort = (): void => {
      void this.abort(new Error("Cursor request aborted"));
    };
    signal.addEventListener("abort", abort, { once: true });
    this.abortCleanup = () => signal.removeEventListener("abort", abort);
    if (signal.aborted) abort();
  }

  async start(): Promise<void> {
    await claimIndependentSession(this);
    if (this.disposed) return;
    const customTools = Object.fromEntries(
      (this.context.tools ?? []).map((tool) => [tool.name, this.createCustomTool(tool)]),
    );
    const model = await cursorModelSelection(
      this.model.id,
      this.options?.reasoning,
      this.options?.apiKey,
    );
    const agentId = cursorProcessAgentId();
    const agentOptions: AgentOptions = {
      ...(this.options?.apiKey ? { apiKey: this.options.apiKey } : {}),
      agentId,
      name: agentId,
      idempotencyKey: agentId,
      model,
      // Keep MCP so pi customTools stay available. Cursor exposes those as the
      // custom-user-tools MCP server, and omitting "mcp" disables that family.
      // webSearch/semSearch have no pi equivalent. Keep shell for Cursor-native
      // command execution. Leave task/await off because those native tools run
      // inside the SDK and never surface as pi tool calls, which leaves the TUI
      // stuck on Working...
      tools: [...CURSOR_ALLOWED_TOOLS],
      local: {
        cwd: process.cwd(),
        settingSources: [],
        customTools,
        // Isolate this pi process from other concurrent Cursor agents in the
        // same workspace. The default SDK store is cwd-scoped and one busy
        // local run blocks every other send() with AgentBusyError.
        store: getProcessCursorStore(),
      },
    };
    const platform = await ensureCursorPlatform(this.options?.apiKey, process.cwd());
    this.agent = platform
      ? await platform.createAgent(agentOptions)
      : await Agent.create(agentOptions);
    if (this.disposed) {
      await this.agent[Symbol.asyncDispose]().catch(() => undefined);
      return;
    }
    this.run = await this.agent.send(buildCursorMessage(this.context), {
      onDelta: ({ update }) => this.handleDelta(update),
      local: { force: true },
    });
    const result = await this.run.wait();
    if (result.status !== "finished") {
      throw new Error(result.error?.message ?? `Cursor run ended with status ${result.status}`);
    }
    if (this.invocations.size > 0) return;
    if (result.usage && this.output) updateUsage(this.output, result.usage);
    if (result.result && this.output && this.output.partial.content.length === 0) {
      this.output.appendText(result.result);
    }
    this.output?.finish("stop");
    await this.dispose();
  }

  resolveToolResults(toolResults: readonly ToolResultMessage[]): void {
    toolResults.forEach((result) => {
      const invocation = this.invocations.get(result.toolCallId);
      if (!invocation) return;
      this.invocations.delete(result.toolCallId);
      pendingByToolCallId.delete(result.toolCallId);
      invocation.resolve(toolResultToSdk(result));
    });
  }

  async fail(error: unknown): Promise<void> {
    this.output?.fail(error, this.currentSignal?.aborted === true);
    await this.dispose(error instanceof Error ? error : new Error(String(error)));
  }

  private createCustomTool(tool: Tool): SDKCustomTool {
    return {
      description: tool.description,
      inputSchema: schemaForTool(tool),
      execute: (args) => this.enqueueTool(tool.name, args),
    };
  }

  private enqueueTool(name: string, args: Record<string, SDKJsonValue>): Promise<SDKJsonValue> {
    const id = `cursor-tool-${randomUUID()}`;
    return new Promise<SDKJsonValue>((resolve, reject) => {
      const invocation: PendingInvocation = { id, session: this, resolve, reject };
      this.invocations.set(id, invocation);
      pendingByToolCallId.set(id, invocation);
      this.output?.appendToolCall({ type: "toolCall", id, name, arguments: args });
      this.scheduleToolBatchFinish();
    });
  }

  private scheduleToolBatchFinish(): void {
    if (this.toolBatchTimer) clearTimeout(this.toolBatchTimer);
    this.toolBatchTimer = setTimeout(() => {
      this.toolBatchTimer = undefined;
      this.output?.finish("toolUse");
      this.detachOutput();
    }, TOOL_BATCH_DELAY_MS);
  }

  private handleDelta(update: InteractionUpdate): void {
    switch (update.type) {
      case "text-delta":
        this.output?.appendText(update.text);
        return;
      case "thinking-delta":
        this.output?.appendThinking(update.text);
        return;
      case "thinking-completed":
        this.output?.endThinking();
        return;
      case "summary":
        this.output?.appendTextBlock(update.summary);
        return;
      case "summary-completed":
        this.output?.appendTextBlock("Summary completed.");
        return;
      case "step-started":
        this.output?.appendTextBlock(`Step ${update.stepId} started`);
        return;
      case "step-completed":
        this.output?.appendTextBlock(
          `Step ${update.stepId} completed (${update.stepDurationMs}ms)`,
        );
        return;
      case "shell-output-delta":
        this.handleShellOutputDelta(update);
        return;
      case "tool-call-started":
      case "partial-tool-call":
        this.handleToolCallStarted(update);
        return;
      case "tool-call-completed":
        this.handleToolCallCompleted(update);
        return;
      case "tool-call-delta":
        this.handleToolCallDelta(update);
        return;
      case "turn-ended":
        this.handleTurnEnded(update);
        return;
      default:
        return;
    }
  }

  private handleShellOutputDelta(update: ShellOutputDeltaUpdate): void {
    const text = shellOutputText(update.event);
    if (text !== undefined) this.output?.appendTextBlock(text);
  }

  private handleToolCallStarted(update: { callId: string; toolCall: ToolCall }): void {
    if (this.startedToolCallIds.has(update.callId)) return;
    this.startedToolCallIds.add(update.callId);
    const text = formatCursorToolCallUpdate(update.toolCall, "started");
    if (text !== undefined) this.output?.appendTextBlock(text);
  }

  private handleToolCallCompleted(update: { callId: string; toolCall: ToolCall }): void {
    if (this.completedToolCallIds.has(update.callId)) return;
    this.completedToolCallIds.add(update.callId);
    const text = formatCursorToolCallUpdate(update.toolCall, "completed");
    if (text !== undefined) this.output?.appendTextBlock(text);
  }

  private handleToolCallDelta(update: ToolCallDeltaUpdate): void {
    const taskUpdate = update.taskUpdate;
    if (taskUpdate.type === "text-delta") {
      this.output?.appendTextBlock(taskUpdate.text);
      return;
    }
    if (taskUpdate.type === "tool-call-started" || taskUpdate.type === "partial-tool-call") {
      this.handleToolCallStarted(taskUpdate);
      return;
    }
    if (taskUpdate.type === "tool-call-completed") {
      this.handleToolCallCompleted(taskUpdate);
      return;
    }
  }

  private handleTurnEnded(update: TurnEndedUpdate): void {
    if (!update.usage || !this.output) return;
    const { inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens } = update.usage;
    updateUsage(this.output, {
      inputTokens,
      outputTokens,
      cacheReadTokens,
      cacheWriteTokens,
      totalTokens: inputTokens + outputTokens,
    });
  }

  private async abort(error: Error): Promise<void> {
    await this.run?.cancel().catch(() => undefined);
    await this.fail(error);
  }

  private detachOutput(): void {
    this.abortCleanup?.();
    this.abortCleanup = undefined;
    this.output = undefined;
    this.currentSignal = undefined;
  }

  private async dispose(reason = new Error("Cursor session disposed")): Promise<void> {
    if (this.disposed) return this.settle;
    this.disposed = true;
    if (this.toolBatchTimer) clearTimeout(this.toolBatchTimer);
    this.invocations.forEach((invocation) => {
      pendingByToolCallId.delete(invocation.id);
      invocation.reject(reason);
    });
    this.invocations.clear();
    this.detachOutput();
    if (independentSessionState.active === this) independentSessionState.active = undefined;
    await this.agent?.[Symbol.asyncDispose]().catch(() => undefined);
    this.resolveSettle();
  }
}

async function claimIndependentSession(session: CursorSession): Promise<void> {
  const previousClaim = independentSessionState.claimQueue;
  const claimGate: { release: () => void } = { release: () => undefined };
  independentSessionState.claimQueue = new Promise<void>((resolve) => {
    claimGate.release = resolve;
  });
  try {
    await previousClaim;
    const previous = independentSessionState.active;
    independentSessionState.active = session;
    if (previous && previous !== session) {
      await Promise.race([
        previous
          .shutdown(new Error("Superseded by a new Cursor request"))
          .then(() => previous.settled),
        delay(independentSessionState.claimTimeoutMs),
      ]);
    }
  } finally {
    claimGate.release();
  }
}

export function streamCursor(
  model: Model<Api>,
  context: Context,
  options?: SimpleStreamOptions,
): AssistantMessageEventStream {
  const output = createCursorOutput(model);
  const toolResults = findToolResults(context);
  const pendingSession = findPendingSession(toolResults);
  const session = pendingSession ?? new CursorSession(context, model, options);
  session.attach(output, options?.signal);

  const operation = pendingSession
    ? Promise.resolve(session.resolveToolResults(toolResults))
    : session.start();
  operation.catch((error: unknown) => session.fail(error));
  return output.stream;
}

export const cursorProviderTestApi = {
  pendingCount: (): number => pendingByToolCallId.size,
  waitForIdle: (): Promise<void> => independentSessionState.active?.settled ?? Promise.resolve(),
  setClaimTimeoutMs(ms = DEFAULT_CLAIM_TIMEOUT_MS): void {
    independentSessionState.claimTimeoutMs = ms;
  },
  formatDiff,
  formatCursorToolCallUpdate,
  shellOutputText,
};
