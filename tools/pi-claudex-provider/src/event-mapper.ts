// Runs with Bun.

import type { AssistantMessage, AssistantMessageEvent, ToolCall } from "@earendil-works/pi-ai";
import { GatewayError } from "./errors.ts";
import { serverMessage, type ServerMessage } from "./protocol.ts";
import { normalizeThoughtResult } from "./thought-result.ts";

type ContentEvent = Exclude<AssistantMessageEvent, { type: "start" | "done" | "error" }>;

interface TerminalContract {
  state: "complete" | "recoverable_error";
  output: "assistant" | "tool_use" | "none";
  code?: "empty_assistant" | "tool_use_without_call";
}

function isToolCall(value: unknown): value is ToolCall {
  return (
    typeof value === "object" && value !== null && "type" in value && value.type === "toolCall"
  );
}

function toolCallAt(
  content: AssistantMessage["content"],
  index: number,
  requestId: string,
): ToolCall {
  const block = content[index];
  if (!isToolCall(block)) {
    throw new GatewayError(`Pi toolcall_start index ${index} has no tool call`, requestId);
  }
  return block;
}

function mapContentEvent(requestId: string, event: ContentEvent): ServerMessage {
  const common = { id: requestId };
  switch (event.type) {
    case "text_start":
    case "thinking_start": {
      return serverMessage(event.type, { ...common, index: event.contentIndex });
    }
    case "text_delta":
    case "toolcall_delta": {
      return serverMessage(event.type, {
        ...common,
        index: event.contentIndex,
        delta: event.delta,
      });
    }
    case "thinking_delta": {
      return serverMessage("thinking_progress", {
        ...common,
        index: event.contentIndex,
      });
    }
    case "text_end": {
      return serverMessage(event.type, {
        ...common,
        index: event.contentIndex,
        content: event.content,
      });
    }
    case "thinking_end": {
      return serverMessage("thinking_result", {
        ...common,
        index: event.contentIndex,
        result: normalizeThoughtResult(event.partial, event.contentIndex, event.content),
      });
    }
    case "toolcall_start": {
      const toolCall = toolCallAt(event.partial.content, event.contentIndex, requestId);
      return serverMessage(event.type, {
        ...common,
        index: event.contentIndex,
        toolCallId: toolCall.id,
        name: toolCall.name,
      });
    }
    case "toolcall_end": {
      return serverMessage(event.type, {
        ...common,
        index: event.contentIndex,
        toolCallId: event.toolCall.id,
        name: event.toolCall.name,
        arguments: event.toolCall.arguments,
      });
    }
  }
}

function hasVisibleAssistantText(message: AssistantMessage): boolean {
  return message.content.some(
    (block) => block.type === "text" && block.text.replaceAll("\u200B", "").trim().length > 0,
  );
}

function hasToolCall(message: AssistantMessage): boolean {
  return message.content.some(isToolCall);
}

function terminalContract(
  reason: Extract<AssistantMessageEvent, { type: "done" }>["reason"],
  message: AssistantMessage,
): TerminalContract {
  if (hasToolCall(message)) {
    return { state: "complete", output: "tool_use" };
  }
  if (hasVisibleAssistantText(message)) {
    return { state: "complete", output: "assistant" };
  }
  if (reason === "toolUse") {
    return { state: "recoverable_error", output: "none", code: "tool_use_without_call" };
  }
  if (reason === "stop" || reason === "length") {
    return { state: "recoverable_error", output: "none", code: "empty_assistant" };
  }
  return { state: "complete", output: "none" };
}

export function mapAssistantEvent(requestId: string, event: AssistantMessageEvent): ServerMessage {
  if (event.type === "start") {
    return serverMessage("start", {
      id: requestId,
      provider: event.partial.provider,
      model: event.partial.model,
      api: event.partial.api,
    });
  }
  if (event.type === "done") {
    return serverMessage("done", {
      id: requestId,
      reason: event.reason,
      message: event.message,
      terminal: terminalContract(event.reason, event.message),
    });
  }
  if (event.type === "error") {
    return serverMessage("error", { id: requestId, reason: event.reason, error: event.error });
  }
  return mapContentEvent(requestId, event);
}
