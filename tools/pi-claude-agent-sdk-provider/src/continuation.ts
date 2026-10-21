// This file runs with Bun.
import type { Message, SystemMessage, ToolResultMessage, UserMessage } from "@earendil-works/pi-ai";
import type { SessionSettings } from "./session.ts";

export interface SessionSnapshot {
  readonly awaitedToolIds: readonly string[];
  readonly closed: boolean;
  readonly replyTimestamp: number | undefined;
  readonly settings: SessionSettings;
}

export type Continuation =
  | { readonly kind: "restart" }
  | {
      readonly kind: "toolResults";
      readonly results: readonly ToolResultMessage[];
      readonly updates: readonly SystemMessage[];
    }
  | {
      readonly kind: "user";
      readonly messages: readonly UserMessage[];
      readonly updates: readonly SystemMessage[];
    };

const RESTART: Continuation = { kind: "restart" };

function toolSignature(settings: SessionSettings): string {
  return JSON.stringify(
    settings.tools.map((tool) => [tool.name, tool.description, tool.parameters]),
  );
}

/** Effort is applied live; every other setting is fixed for the life of an SDK session. */
export function sameSessionSettings(left: SessionSettings, right: SessionSettings): boolean {
  return (
    left.model === right.model &&
    left.cwd === right.cwd &&
    left.systemPrompt === right.systemPrompt &&
    left.thinking === right.thinking &&
    toolSignature(left) === toolSignature(right)
  );
}

function isToolResult(message: Message): message is ToolResultMessage {
  return message.role === "toolResult";
}

function isUser(message: Message): message is UserMessage {
  return message.role === "user";
}

function isSystem(message: Message): message is SystemMessage {
  return message.role === "system";
}

function toolResults(tail: readonly Message[]): ToolResultMessage[] {
  return tail.flatMap((message) => (isToolResult(message) ? [message] : []));
}

function answersExactly(tail: readonly Message[], ids: readonly string[]): boolean {
  const results = toolResults(tail);
  const answered = new Set(results.map((result) => result.toolCallId));
  return (
    results.length === tail.length &&
    answered.size === ids.length &&
    ids.every((id) => answered.has(id))
  );
}

/**
 * Pi inserts transient system messages (effort or goal guidance) and drops them a turn later,
 * so positions shift; the provider's own last reply is the stable anchor.
 */
function tailAfterReply(
  session: SessionSnapshot,
  messages: readonly Message[],
): readonly Message[] | undefined {
  const index = messages.findLastIndex(
    (message) => message.role === "assistant" && message.timestamp === session.replyTimestamp,
  );
  return index === -1 ? undefined : messages.slice(index + 1);
}

function continuationFor(tail: readonly Message[], awaited: readonly string[]): Continuation {
  const updates = tail.flatMap((message) => (isSystem(message) ? [message] : []));
  const rest = tail.filter((message) => !isSystem(message));
  if (awaited.length > 0) {
    return answersExactly(rest, awaited)
      ? { kind: "toolResults", results: toolResults(rest), updates }
      : RESTART;
  }
  const users = rest.flatMap((message) => (isUser(message) ? [message] : []));
  return users.length > 0 && users.length === rest.length
    ? { kind: "user", messages: users, updates }
    : RESTART;
}

/**
 * Continue only when pi's transcript extends this session exactly: results for every tool
 * call it is waiting on, or new user input after a finished reply, plus any system updates.
 * Anything else (compaction, branching, steering mixed into tool results, changed base
 * instructions or tools) restarts.
 */
export function planContinuation(
  session: SessionSnapshot | undefined,
  messages: readonly Message[],
  settings: SessionSettings,
): Continuation {
  if (session === undefined || session.closed || !sameSessionSettings(session.settings, settings)) {
    return RESTART;
  }
  const tail = tailAfterReply(session, messages);
  return tail === undefined ? RESTART : continuationFor(tail, session.awaitedToolIds);
}
