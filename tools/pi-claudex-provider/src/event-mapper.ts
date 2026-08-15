import type { AssistantMessage, AssistantMessageEvent, ToolCall } from "@earendil-works/pi-ai";
import { GatewayError } from "./errors.ts";
import { serverMessage, type ServerMessage } from "./protocol.ts";

type ContentEvent = Exclude<AssistantMessageEvent, { type: "start" | "done" | "error" }>;

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
    case "thinking_delta":
    case "toolcall_delta": {
      return serverMessage(event.type, {
        ...common,
        index: event.contentIndex,
        delta: event.delta,
      });
    }
    case "text_end":
    case "thinking_end": {
      return serverMessage(event.type, {
        ...common,
        index: event.contentIndex,
        content: event.content,
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
    return serverMessage("done", { id: requestId, reason: event.reason, message: event.message });
  }
  if (event.type === "error") {
    return serverMessage("error", { id: requestId, reason: event.reason, error: event.error });
  }
  return mapContentEvent(requestId, event);
}
