import { randomUUID } from "node:crypto";
import {
  Agent,
  type InteractionUpdate,
  type Run,
  type SDKAgent,
  type SDKCustomTool,
  type SDKJsonValue,
  type TokenUsage,
  type ToolName,
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
const CURSOR_ALLOWED_TOOLS: readonly ToolName[] = ["mcp", "webSearch", "semSearch", "shell"];

interface IndependentSessionState {
  active: CursorSession | undefined;
  claimQueue: Promise<void>;
}

const independentSessionState: IndependentSessionState = {
  active: undefined,
  claimQueue: Promise.resolve(),
};

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
    this.agent = await Agent.create({
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
    });
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
    if (update.type === "text-delta") this.output?.appendText(update.text);
    if (update.type === "thinking-delta") this.output?.appendThinking(update.text);
    if (update.type === "thinking-completed") this.output?.endThinking();
    if (update.type !== "turn-ended" || !update.usage || !this.output) return;
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
      await previous.shutdown(new Error("Superseded by a new Cursor request"));
      await previous.settled;
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
};
