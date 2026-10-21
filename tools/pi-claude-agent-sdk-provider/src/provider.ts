// This file runs with Bun.
import type { EffortLevel } from "@anthropic-ai/claude-agent-sdk";
import {
  getCurrentTools,
  getInitialSystemMessage,
  getSystemMessageText,
  normalizeContext,
  type Api,
  type AssistantMessageEventStream,
  type Context,
  type Message,
  type Model,
  type SimpleStreamOptions,
  type SystemMessage,
  type ThinkingLevel,
  type ToolResultMessage,
} from "@earendil-works/pi-ai";
import { planContinuation } from "./continuation.ts";
import { initialSdkMessage, systemUpdateText, userMessagesToSdk } from "./sdk-input.ts";
import {
  ClaudeSession,
  type SessionEvent,
  type SessionSettings,
  type StartQuery,
} from "./session.ts";
import { TurnOutput } from "./stream-output.ts";
import { toCallToolResult } from "./tool-bridge.ts";
import { drainTurn, type DrainInput, type TurnEnd } from "./turn.ts";

export interface ProviderRuntime {
  readonly cwd: () => string;
  readonly printMode: boolean;
  readonly startQuery?: StartQuery | undefined;
}

interface TurnRequest {
  readonly context: Context;
  readonly model: Model<Api>;
  readonly options: SimpleStreamOptions | undefined;
  readonly output: TurnOutput;
  readonly runtime: ProviderRuntime;
}

interface PreparedTurn {
  readonly messages: readonly Message[];
  readonly session: ClaudeSession;
}

const DEFAULT_SESSION_KEY = "pi-claude-sdk:default";
// Releases the Claude Code child of an abandoned conversation without blocking exit.
const IDLE_CLOSE_MS = 15 * 60 * 1000;
const PRINT_FLAGS: ReadonlySet<string> = new Set(["-p", "--print"]);
const EFFORT_BY_LEVEL: Record<ThinkingLevel, EffortLevel> = {
  minimal: "low",
  low: "low",
  medium: "medium",
  high: "high",
  xhigh: "xhigh",
  max: "max",
};

const sessions = new Map<string, ClaudeSession>();
const idleTimers = new Map<string, NodeJS.Timeout>();

export function defaultRuntime(): ProviderRuntime {
  return { cwd: () => process.cwd(), printMode: process.argv.some((arg) => PRINT_FLAGS.has(arg)) };
}

function settingsFor(request: TurnRequest, messages: readonly Message[]): SessionSettings {
  const reasoning = request.options?.reasoning;
  const initial = getInitialSystemMessage(messages);
  return {
    cwd: request.runtime.cwd(),
    effort: reasoning === undefined ? undefined : EFFORT_BY_LEVEL[reasoning],
    model: request.model.id,
    // Later system messages are delivered as updates; only the leading prompt is fixed.
    systemPrompt: initial === undefined ? "" : getSystemMessageText(initial),
    thinking: reasoning !== undefined,
    tools: getCurrentTools(messages),
  };
}

function closeSession(key: string, reason: string): void {
  sessions.get(key)?.close(reason);
  sessions.delete(key);
  clearTimeout(idleTimers.get(key));
  idleTimers.delete(key);
}

function scheduleIdleClose(key: string): void {
  clearTimeout(idleTimers.get(key));
  const timer = setTimeout(() => {
    closeSession(key, "Claude Agent SDK session idle");
  }, IDLE_CLOSE_MS);
  timer.unref();
  idleTimers.set(key, timer);
}

function startSession(
  key: string,
  request: TurnRequest,
  messages: readonly Message[],
): ClaudeSession {
  closeSession(key, "Claude Agent SDK session restarted");
  const session = new ClaudeSession(settingsFor(request, messages), request.runtime.startQuery);
  sessions.set(key, session);
  session.send(initialSdkMessage(messages));
  return session;
}

/** System updates issued alongside tool results ride on the last result the model reads. */
function settleToolResults(
  session: ClaudeSession,
  results: readonly ToolResultMessage[],
  updates: readonly SystemMessage[],
): void {
  const notes = updates.map((update) => systemUpdateText(update));
  const last = results.length - 1;
  for (const [index, result] of results.entries()) {
    session.pending.settle(
      result.toolCallId,
      toCallToolResult(result, index === last ? notes : []),
    );
  }
}

async function prepareTurn(key: string, request: TurnRequest): Promise<PreparedTurn> {
  const { messages } = normalizeContext(request.context);
  const settings = settingsFor(request, messages);
  const existing = sessions.get(key);
  const plan = planContinuation(existing, messages, settings);
  if (existing === undefined || plan.kind === "restart") {
    return { messages, session: startSession(key, request, messages) };
  }
  if (settings.effort !== undefined && settings.effort !== existing.effort) {
    await existing.setEffort(settings.effort);
  }
  if (plan.kind === "toolResults") {
    settleToolResults(existing, plan.results, plan.updates);
  } else {
    existing.send(userMessagesToSdk(plan.messages, plan.updates));
  }
  return { messages, session: existing };
}

function streamHook(request: TurnRequest): DrainInput["onEvent"] {
  const hook = request.options?.onProviderStreamEvent;
  return hook === undefined
    ? undefined
    : async (event: SessionEvent): Promise<void> => hook(event, request.model);
}

function isAborted(request: TurnRequest): boolean {
  return request.options?.signal?.aborted ?? false;
}

function completeTurn(key: string, turn: PreparedTurn, end: TurnEnd, request: TurnRequest): void {
  const { output } = request;
  if (end.kind === "error") {
    closeSession(key, "Claude Agent SDK turn failed");
    output.fail(end.error, isAborted(request));
    return;
  }
  turn.session.awaitedToolIds = end.kind === "toolUse" ? end.toolIds : [];
  turn.session.replyTimestamp = output.partial.timestamp;
  output.finish(end.kind);
  if (end.kind === "stop" && request.runtime.printMode) {
    closeSession(key, "pi print mode finished");
    return;
  }
  scheduleIdleClose(key);
}

async function runTurn(request: TurnRequest): Promise<void> {
  const key = request.options?.sessionId ?? DEFAULT_SESSION_KEY;
  const signal = request.options?.signal;
  const abort = (): void => {
    closeSession(key, "pi aborted the turn");
  };
  signal?.addEventListener("abort", abort, { once: true });
  try {
    const turn = await prepareTurn(key, request);
    const end = await drainTurn({
      events: turn.session.events,
      output: request.output,
      onEvent: streamHook(request),
    });
    completeTurn(key, turn, end, request);
  } catch (error) {
    closeSession(key, "Claude Agent SDK turn failed");
    request.output.fail(error, isAborted(request));
  } finally {
    signal?.removeEventListener("abort", abort);
  }
}

export function streamClaudeSdk(
  model: Model<Api>,
  context: Context,
  options?: SimpleStreamOptions,
  runtime: ProviderRuntime = defaultRuntime(),
): AssistantMessageEventStream {
  const output = new TurnOutput(model);
  if (options?.signal?.aborted === true) {
    output.fail(new Error("Request was aborted before it started"), true);
    return output.stream;
  }
  // RunTurn reports every failure through the output stream.
  runTurn({ context, model, options, output, runtime }).catch(() => null);
  return output.stream;
}

export function closeAllSessions(reason: string): void {
  for (const key of sessions.keys()) {
    closeSession(key, reason);
  }
}
