import { randomUUID } from "node:crypto";
import {
  Agent,
  type InteractionUpdate,
  type Run,
  type SDKAgent,
  type SDKCustomTool,
  type SDKJsonValue,
  type TokenUsage,
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
import { createCursorOutput, type CursorOutput } from "./stream-output.ts";

interface PendingInvocation {
  readonly id: string;
  readonly session: CursorSession;
  resolve(result: SDKJsonValue): void;
  reject(error: Error): void;
}

const pendingByToolCallId = new Map<string, PendingInvocation>();
const TOOL_BATCH_DELAY_MS = 0;

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

  constructor(context: Context, model: Model<Api>, options: SimpleStreamOptions | undefined) {
    this.context = context;
    this.model = model;
    this.options = options;
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
    const customTools = Object.fromEntries(
      (this.context.tools ?? []).map((tool) => [tool.name, this.createCustomTool(tool)]),
    );
    this.agent = await Agent.create({
      ...(this.options?.apiKey ? { apiKey: this.options.apiKey } : {}),
      model: { id: this.model.id },
      local: {
        cwd: process.cwd(),
        settingSources: [],
        customTools,
      },
    });
    if (this.disposed) {
      await this.agent[Symbol.asyncDispose]().catch(() => undefined);
      return;
    }
    this.run = await this.agent.send(buildCursorMessage(this.context), {
      onDelta: ({ update }) => this.handleDelta(update),
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
    if (this.disposed) return;
    this.disposed = true;
    if (this.toolBatchTimer) clearTimeout(this.toolBatchTimer);
    this.invocations.forEach((invocation) => {
      pendingByToolCallId.delete(invocation.id);
      invocation.reject(reason);
    });
    this.invocations.clear();
    this.detachOutput();
    await this.agent?.[Symbol.asyncDispose]().catch(() => undefined);
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
};
