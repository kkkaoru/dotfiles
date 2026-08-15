import type {
  Api,
  AssistantMessage,
  AssistantMessageEventStream,
  Model,
  ToolCall,
} from "@earendil-works/pi-ai";
import { createAssistantMessageEventStream } from "@earendil-works/pi-ai";

export interface CursorOutput {
  readonly stream: AssistantMessageEventStream;
  readonly partial: AssistantMessage;
  appendText(delta: string): void;
  appendThinking(delta: string): void;
  appendToolCall(toolCall: ToolCall): void;
  finish(reason: "stop" | "toolUse"): void;
  fail(error: unknown, aborted: boolean): void;
}

function createInitialMessage(model: Model<Api>): AssistantMessage {
  return {
    role: "assistant",
    content: [],
    api: model.api,
    provider: model.provider,
    model: model.id,
    usage: {
      input: 0,
      output: 0,
      cacheRead: 0,
      cacheWrite: 0,
      totalTokens: 0,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
    },
    stopReason: "pending",
    timestamp: Date.now(),
  };
}

export function createCursorOutput(model: Model<Api>): CursorOutput {
  const stream = createAssistantMessageEventStream();
  const partial = createInitialMessage(model);
  let finished = false;

  stream.push({ type: "start", partial });

  const appendText = (delta: string): void => {
    if (finished || delta.length === 0) return;
    const previous = partial.content.at(-1);
    if (previous?.type === "text") {
      previous.text += delta;
      stream.push({ type: "text_delta", contentIndex: partial.content.length - 1, delta, partial });
      return;
    }
    partial.content.push({ type: "text", text: delta });
    const contentIndex = partial.content.length - 1;
    stream.push({ type: "text_start", contentIndex, partial });
    stream.push({ type: "text_delta", contentIndex, delta, partial });
  };

  const appendThinking = (delta: string): void => {
    if (finished || delta.length === 0) return;
    const previous = partial.content.at(-1);
    if (previous?.type === "thinking") {
      previous.thinking += delta;
      stream.push({
        type: "thinking_delta",
        contentIndex: partial.content.length - 1,
        delta,
        partial,
      });
      return;
    }
    partial.content.push({ type: "thinking", thinking: delta });
    const contentIndex = partial.content.length - 1;
    stream.push({ type: "thinking_start", contentIndex, partial });
    stream.push({ type: "thinking_delta", contentIndex, delta, partial });
  };

  const appendToolCall = (toolCall: ToolCall): void => {
    if (finished) return;
    partial.content.push(toolCall);
    const contentIndex = partial.content.length - 1;
    stream.push({ type: "toolcall_start", contentIndex, partial });
    stream.push({
      type: "toolcall_delta",
      contentIndex,
      delta: JSON.stringify(toolCall.arguments),
      partial,
    });
    stream.push({ type: "toolcall_end", contentIndex, toolCall, partial });
  };

  const finish = (reason: "stop" | "toolUse"): void => {
    if (finished) return;
    finished = true;
    partial.stopReason = reason;
    stream.push({ type: "done", reason, message: partial });
    stream.end();
  };

  const fail = (error: unknown, aborted: boolean): void => {
    if (finished) return;
    finished = true;
    partial.stopReason = aborted ? "aborted" : "error";
    partial.errorMessage = error instanceof Error ? error.message : String(error);
    stream.push({ type: "error", reason: partial.stopReason, error: partial });
    stream.end();
  };

  return { stream, partial, appendText, appendThinking, appendToolCall, finish, fail };
}
