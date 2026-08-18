// This file runs with Bun.
import type {
  Api,
  AssistantMessage,
  AssistantMessageEventStream,
  Model,
  ThinkingContent,
  ToolCall,
} from "@earendil-works/pi-ai";
import { createAssistantMessageEventStream } from "@earendil-works/pi-ai";

export interface CursorOutput {
  readonly stream: AssistantMessageEventStream;
  readonly partial: AssistantMessage;
  appendText(delta: string): void;
  appendTextBlock(text: string): void;
  appendThinking(delta: string): void;
  endThinking(): void;
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

interface OutputState {
  finished: boolean;
  openText: number | undefined;
  openThinking: { contentIndex: number; block: ThinkingContent } | undefined;
}

export function createCursorOutput(model: Model<Api>): CursorOutput {
  const stream = createAssistantMessageEventStream();
  const partial = createInitialMessage(model);
  const state: OutputState = { finished: false, openText: undefined, openThinking: undefined };

  stream.push({ type: "start", partial });

  const endThinking = (): void => {
    if (state.finished || !state.openThinking) return;
    const { contentIndex, block } = state.openThinking;
    state.openThinking = undefined;
    stream.push({
      type: "thinking_end",
      contentIndex,
      content: block.thinking,
      partial,
    });
  };

  const appendText = (delta: string): void => {
    if (state.finished || delta.length === 0) return;
    endThinking();
    if (state.openText !== undefined) {
      const previous = partial.content[state.openText];
      if (previous?.type === "text") {
        previous.text += delta;
        stream.push({ type: "text_delta", contentIndex: state.openText, delta, partial });
        return;
      }
      state.openText = undefined;
    }
    partial.content.push({ type: "text", text: delta });
    const contentIndex = partial.content.length - 1;
    state.openText = contentIndex;
    stream.push({ type: "text_start", contentIndex, partial });
    stream.push({ type: "text_delta", contentIndex, delta, partial });
  };

  const appendTextBlock = (text: string): void => {
    if (state.finished || text.length === 0) return;
    endThinking();
    state.openText = undefined;
    partial.content.push({ type: "text", text });
    const contentIndex = partial.content.length - 1;
    stream.push({ type: "text_start", contentIndex, partial });
    stream.push({ type: "text_delta", contentIndex, delta: text, partial });
  };

  const appendThinking = (delta: string): void => {
    if (state.finished || delta.length === 0) return;
    state.openText = undefined;
    if (state.openThinking) {
      state.openThinking.block.thinking += delta;
      stream.push({
        type: "thinking_delta",
        contentIndex: state.openThinking.contentIndex,
        delta,
        partial,
      });
      return;
    }
    const block: ThinkingContent = { type: "thinking", thinking: delta };
    partial.content.push(block);
    const contentIndex = partial.content.length - 1;
    state.openThinking = { contentIndex, block };
    stream.push({ type: "thinking_start", contentIndex, partial });
    stream.push({ type: "thinking_delta", contentIndex, delta, partial });
  };

  const appendToolCall = (toolCall: ToolCall): void => {
    if (state.finished) return;
    endThinking();
    state.openText = undefined;
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
    if (state.finished) return;
    endThinking();
    state.finished = true;
    partial.stopReason = reason;
    stream.push({ type: "done", reason, message: partial });
    stream.end();
  };

  const fail = (error: unknown, aborted: boolean): void => {
    if (state.finished) return;
    endThinking();
    state.finished = true;
    partial.stopReason = aborted ? "aborted" : "error";
    partial.errorMessage = error instanceof Error ? error.message : String(error);
    stream.push({ type: "error", reason: partial.stopReason, error: partial });
    stream.end();
  };

  return {
    stream,
    partial,
    appendText,
    appendTextBlock,
    appendThinking,
    endThinking,
    appendToolCall,
    finish,
    fail,
  };
}
