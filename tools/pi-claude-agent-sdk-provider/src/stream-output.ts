// This file runs with Bun.
import {
  createAssistantMessageEventStream,
  parseStreamingJson,
  type Api,
  type AssistantMessage,
  type AssistantMessageEventStream,
  type JsonObject,
  type Model,
  type Usage,
} from "@earendil-works/pi-ai";

export type TurnStop = "stop" | "toolUse";
export type TokenUsage = Omit<Usage, "cost" | "totalTokens">;

interface OpenBlock {
  readonly contentIndex: number;
  readonly kind: "text" | "thinking" | "toolCall";
}

// Plan usage is not billed per token, so pi shows no cost for this provider.
const ZERO_COST: Usage["cost"] = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 };

function initialMessage(model: Model<Api>): AssistantMessage {
  return {
    role: "assistant",
    content: [],
    api: model.api,
    provider: model.provider,
    model: model.id,
    usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: ZERO_COST },
    stopReason: "pending",
    timestamp: Date.now(),
  };
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Builds one pi assistant message and emits balanced stream events for it. */
export class TurnOutput {
  readonly stream: AssistantMessageEventStream = createAssistantMessageEventStream();
  readonly partial: AssistantMessage;
  #finished = false;
  #open: OpenBlock | undefined;
  #toolInput = "";

  constructor(model: Model<Api>) {
    this.partial = initialMessage(model);
    this.stream.push({ type: "start", partial: this.partial });
  }

  get finished(): boolean {
    return this.#finished;
  }

  appendText(delta: string): void {
    if (this.#finished || delta.length === 0) {
      return;
    }
    const contentIndex = this.#openBlock("text");
    const current = this.partial.content[contentIndex];
    if (current?.type === "text") {
      current.text += delta;
    }
    this.stream.push({ type: "text_delta", contentIndex, delta, partial: this.partial });
  }

  appendThinking(delta: string): void {
    if (this.#finished || delta.length === 0) {
      return;
    }
    const contentIndex = this.#openBlock("thinking");
    const current = this.partial.content[contentIndex];
    if (current?.type === "thinking") {
      current.thinking += delta;
    }
    this.stream.push({ type: "thinking_delta", contentIndex, delta, partial: this.partial });
  }

  closeBlock(): void {
    const open = this.#open;
    this.#open = undefined;
    const current = open === undefined ? undefined : this.partial.content[open.contentIndex];
    if (open === undefined || current === undefined) {
      return;
    }
    if (current.type === "text") {
      this.stream.push({
        type: "text_end",
        contentIndex: open.contentIndex,
        content: current.text,
        partial: this.partial,
      });
    } else if (current.type === "thinking") {
      this.stream.push({
        type: "thinking_end",
        contentIndex: open.contentIndex,
        content: current.thinking,
        partial: this.partial,
      });
    } else if (current.type === "toolCall") {
      this.#endToolCall(open.contentIndex);
    }
  }

  startToolCall(id: string, name: string): void {
    this.closeBlock();
    this.partial.content.push({ type: "toolCall", id, name, arguments: {} });
    const contentIndex = this.partial.content.length - 1;
    this.#open = { contentIndex, kind: "toolCall" };
    this.#toolInput = "";
    this.stream.push({ type: "toolcall_start", contentIndex, partial: this.partial });
  }

  appendToolInput(delta: string): void {
    if (this.#open?.kind !== "toolCall" || delta.length === 0) {
      return;
    }
    this.#toolInput += delta;
    const { contentIndex } = this.#open;
    this.stream.push({ type: "toolcall_delta", contentIndex, delta, partial: this.partial });
  }

  setUsage(usage: TokenUsage): void {
    const totalTokens = usage.input + usage.output + usage.cacheRead + usage.cacheWrite;
    this.partial.usage = { ...usage, totalTokens, cost: ZERO_COST };
  }

  finish(reason: TurnStop): void {
    if (this.#finished) {
      return;
    }
    this.closeBlock();
    this.#finished = true;
    this.partial.stopReason = reason;
    this.stream.push({ type: "done", reason, message: this.partial });
    this.stream.end();
  }

  fail(error: unknown, aborted: boolean): void {
    if (this.#finished) {
      return;
    }
    this.closeBlock();
    this.#finished = true;
    this.partial.stopReason = aborted ? "aborted" : "error";
    this.partial.errorMessage = errorText(error);
    this.stream.push({ type: "error", reason: this.partial.stopReason, error: this.partial });
    this.stream.end();
  }

  #openBlock(kind: "text" | "thinking"): number {
    if (this.#open?.kind === kind) {
      return this.#open.contentIndex;
    }
    this.closeBlock();
    this.partial.content.push(
      kind === "text" ? { type: "text", text: "" } : { type: "thinking", thinking: "" },
    );
    const contentIndex = this.partial.content.length - 1;
    this.#open = { contentIndex, kind };
    const type = kind === "text" ? "text_start" : "thinking_start";
    this.stream.push({ type, contentIndex, partial: this.partial });
    return contentIndex;
  }

  #endToolCall(contentIndex: number): void {
    const current = this.partial.content[contentIndex];
    if (current?.type !== "toolCall") {
      return;
    }
    current.arguments = parseStreamingJson<JsonObject>(this.#toolInput);
    this.stream.push({
      type: "toolcall_end",
      contentIndex,
      toolCall: current,
      partial: this.partial,
    });
  }
}
