// This file runs with Bun.
import type { SDKMessage, SDKRateLimitInfo } from "@anthropic-ai/claude-agent-sdk";
import type { SessionEvent } from "./session.ts";
import type { TokenUsage, TurnOutput } from "./stream-output.ts";
import { piToolName } from "./tool-bridge.ts";

type StreamEvent = Extract<SDKMessage, { type: "stream_event" }>["event"];
type ResultMessage = Extract<SDKMessage, { type: "result" }>;
type BlockKind = "skip" | "text" | "thinking" | "tool";

export type TurnEnd =
  | { readonly kind: "error"; readonly error: unknown }
  | { readonly kind: "stop" }
  | { readonly kind: "toolUse"; readonly toolIds: readonly string[] };

interface TurnState {
  readonly blocks: Map<number, BlockKind>;
  readonly output: TurnOutput;
  readonly toolIds: string[];
  rateLimit: SDKRateLimitInfo | undefined;
  stopReason: string | null;
}

interface ApiUsage {
  readonly cache_creation_input_tokens?: number | null;
  readonly cache_read_input_tokens?: number | null;
  readonly input_tokens?: number | null;
  readonly output_tokens?: number | null;
}

type Handler<Kind extends StreamEvent["type"]> = (
  state: TurnState,
  event: Extract<StreamEvent, { type: Kind }>,
) => void;

function toUsage(usage: ApiUsage): TokenUsage {
  return {
    input: usage.input_tokens ?? 0,
    output: usage.output_tokens ?? 0,
    cacheRead: usage.cache_read_input_tokens ?? 0,
    cacheWrite: usage.cache_creation_input_tokens ?? 0,
  };
}

function blockKind(
  state: TurnState,
  block: Extract<StreamEvent, { type: "content_block_start" }>["content_block"],
): BlockKind {
  if (block.type === "text" || block.type === "thinking") {
    return block.type;
  }
  const name = block.type === "tool_use" ? piToolName(block.name) : undefined;
  if (block.type !== "tool_use" || name === undefined) {
    return "skip";
  }
  state.output.startToolCall(block.id, name);
  state.toolIds.push(block.id);
  return "tool";
}

const onMessageStart: Handler<"message_start"> = (state, event) => {
  state.blocks.clear();
  state.stopReason = null;
  state.output.setUsage(toUsage(event.message.usage));
};

const onBlockStart: Handler<"content_block_start"> = (state, event) => {
  state.output.closeBlock();
  state.blocks.set(event.index, blockKind(state, event.content_block));
};

const onBlockDelta: Handler<"content_block_delta"> = (state, event) => {
  const { delta } = event;
  if (delta.type === "text_delta") {
    state.output.appendText(delta.text);
  } else if (delta.type === "thinking_delta") {
    state.output.appendThinking(delta.thinking);
  } else if (delta.type === "input_json_delta" && state.blocks.get(event.index) === "tool") {
    state.output.appendToolInput(delta.partial_json);
  }
};

const onBlockStop: Handler<"content_block_stop"> = (state) => {
  state.output.closeBlock();
};

const onMessageDelta: Handler<"message_delta"> = (state, event) => {
  state.stopReason = event.delta.stop_reason;
  state.output.setUsage(toUsage(event.usage));
};

// Claude Code calls the pi tools right after this point; pi runs them and answers next turn.
function messageStopEnd(state: TurnState): TurnEnd | undefined {
  return state.stopReason === "tool_use" && state.toolIds.length > 0
    ? { kind: "toolUse", toolIds: [...state.toolIds] }
    : undefined;
}

const HANDLERS: { readonly [Kind in StreamEvent["type"]]?: Handler<Kind> } = {
  message_start: onMessageStart,
  content_block_start: onBlockStart,
  content_block_delta: onBlockDelta,
  content_block_stop: onBlockStop,
  message_delta: onMessageDelta,
};

function dispatch<Kind extends StreamEvent["type"]>(
  state: TurnState,
  event: Extract<StreamEvent, { type: Kind }>,
): void {
  const handler: Handler<Kind> | undefined = HANDLERS[event.type];
  handler?.(state, event);
}

function handleStreamEvent(state: TurnState, event: StreamEvent): TurnEnd | undefined {
  if (event.type === "message_stop") {
    return messageStopEnd(state);
  }
  dispatch(state, event);
  return undefined;
}

/** The API reports an exhausted plan window as "out of extra usage"; name the window instead. */
function rateLimitNote(info: SDKRateLimitInfo | undefined): string {
  if (info === undefined) {
    return "";
  }
  const resets =
    info.resetsAt === undefined ? "" : `, resets ${new Date(info.resetsAt * 1000).toISOString()}`;
  return ` [plan ${info.rateLimitType ?? "limit"}: ${info.status}${resets}]`;
}

function resultEnd(state: TurnState, message: ResultMessage): TurnEnd {
  if (message.subtype === "success" && !message.is_error) {
    return { kind: "stop" };
  }
  const detail = message.subtype === "success" ? message.result : message.errors.join("; ");
  const note = rateLimitNote(state.rateLimit);
  return {
    kind: "error",
    error: new Error(`Claude Agent SDK ${message.subtype}: ${detail}${note}`),
  };
}

function handleEvent(state: TurnState, event: SessionEvent): TurnEnd | undefined {
  if (event.type === "pump_failure") {
    return { kind: "error", error: event.error };
  }
  if (event.type === "result") {
    return resultEnd(state, event);
  }
  if (event.type === "rate_limit_event") {
    state.rateLimit = event.rate_limit_info;
  }
  return event.type === "stream_event" && event.parent_tool_use_id === null
    ? handleStreamEvent(state, event.event)
    : undefined;
}

export interface DrainInput {
  readonly events: AsyncIterable<SessionEvent>;
  readonly output: TurnOutput;
  readonly onEvent?: ((event: SessionEvent) => Promise<void> | void) | undefined;
}

/** Streams one pi turn; buffered events after the boundary stay queued for the next turn. */
export async function drainTurn(input: DrainInput): Promise<TurnEnd> {
  const state: TurnState = {
    blocks: new Map(),
    output: input.output,
    toolIds: [],
    rateLimit: undefined,
    stopReason: null,
  };
  for await (const event of input.events) {
    await input.onEvent?.(event);
    const end = handleEvent(state, event);
    if (end !== undefined) {
      return end;
    }
  }
  return {
    kind: "error",
    error: new Error("Claude Agent SDK session ended before the turn finished"),
  };
}
