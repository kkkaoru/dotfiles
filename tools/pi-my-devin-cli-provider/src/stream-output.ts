// This file runs with Bun.
import type {
  Api,
  AssistantMessage,
  AssistantMessageEventStream,
  Model,
  ThinkingContent,
} from "@earendil-works/pi-ai";
import { createAssistantMessageEventStream } from "@earendil-works/pi-ai";

export interface DevinOutput {
  readonly partial: AssistantMessage;
  readonly stream: AssistantMessageEventStream;
  appendText(delta: string): void;
  appendTextBlock(text: string): void;
  appendThinking(delta: string): void;
  fail(error: unknown, aborted: boolean): void;
  finish(): void;
}

interface OutputState {
  finished: boolean;
  openText: number | undefined;
  openThinking: { contentIndex: number; block: ThinkingContent } | undefined;
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

export function createDevinOutput(model: Model<Api>): DevinOutput {
  const stream: AssistantMessageEventStream = createAssistantMessageEventStream();
  const partial: AssistantMessage = createInitialMessage(model);
  const state: OutputState = { finished: false, openText: undefined, openThinking: undefined };

  stream.push({ type: "start", partial });

  const endThinking = (): void => {
    if (state.finished || !state.openThinking) return;
    const { contentIndex, block } = state.openThinking;
    state.openThinking = undefined;
    stream.push({ type: "thinking_end", contentIndex, content: block.thinking, partial });
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
    const contentIndex: number = partial.content.length - 1;
    state.openText = contentIndex;
    stream.push({ type: "text_start", contentIndex, partial });
    stream.push({ type: "text_delta", contentIndex, delta, partial });
  };

  const appendTextBlock = (text: string): void => {
    if (state.finished || text.length === 0) return;
    endThinking();
    state.openText = undefined;
    partial.content.push({ type: "text", text });
    const contentIndex: number = partial.content.length - 1;
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
    const contentIndex: number = partial.content.length - 1;
    state.openThinking = { contentIndex, block };
    stream.push({ type: "thinking_start", contentIndex, partial });
    stream.push({ type: "thinking_delta", contentIndex, delta, partial });
  };

  const finish = (): void => {
    if (state.finished) return;
    endThinking();
    state.finished = true;
    partial.stopReason = "stop";
    stream.push({ type: "done", reason: "stop", message: partial });
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

  return { stream, partial, appendText, appendTextBlock, appendThinking, finish, fail };
}
