// This file runs with bun.
import { createAssistantMessageEventStream } from "@earendil-works/pi-ai";
import type {
  Api,
  AssistantMessage,
  JsonObject,
  Model,
  SimpleStreamOptions,
  TextContent,
  ThinkingContent,
  ToolCall,
  TranscriptContext,
  Usage,
} from "@earendil-works/pi-ai";

import { findCli, runCli, type RunCliOptions } from "./cli.ts";
import { INSTALL_HINT } from "./constants.ts";
import { isRecord, parseJson } from "./json.ts";
import { buildBody } from "./request.ts";

export interface GeminiCliStreamOptions extends SimpleStreamOptions {
  /** Test seam: the resolved CLI path. */
  binary?: string;
  /** Test seam: replaces the child process. */
  run?: typeof runCli;
}

export interface ParsedToolCall {
  id: string;
  name: string;
  arguments: JsonObject;
}

export interface InteractionOutput {
  text: string;
  thinking: string;
  toolCalls: ParsedToolCall[];
  usage: Usage;
}

function emptyUsage(): Usage {
  return {
    cacheRead: 0,
    cacheWrite: 0,
    input: 0,
    output: 0,
    totalTokens: 0,
    cost: { cacheRead: 0, cacheWrite: 0, input: 0, output: 0, total: 0 },
  };
}

function count(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function firstCount(record: JsonObject, keys: readonly string[]): number | undefined {
  for (const key of keys) {
    const found = count(record[key]);
    if (found !== undefined) {
      return found;
    }
  }
  return undefined;
}

function usageOf(value: unknown): Usage {
  const usage = emptyUsage();
  if (!isRecord(value)) {
    return usage;
  }
  const input = firstCount(value, ["input_tokens", "total_input_tokens"]) ?? 0;
  const cached = firstCount(value, ["cached_tokens", "total_cached_tokens"]) ?? 0;
  const output = firstCount(value, ["output_tokens", "total_output_tokens"]) ?? 0;
  // Gemini counts cached prompt tokens inside the prompt total; pi keeps them in cacheRead.
  usage.cacheRead = cached;
  usage.input = Math.max(0, input - cached);
  usage.output = output;
  usage.totalTokens = usage.input + usage.output + usage.cacheRead + usage.cacheWrite;
  return usage;
}

function interactionOf(value: unknown): JsonObject | undefined {
  if (!isRecord(value)) {
    return undefined;
  }
  const nested = value["interaction"];
  return isRecord(nested) ? nested : value;
}

function stepsOf(value: unknown): unknown[] {
  const interaction = interactionOf(value);
  const steps = interaction?.["steps"];
  return Array.isArray(steps) ? steps : [];
}

function textOfStep(step: unknown): string {
  const content = isRecord(step) ? step["content"] : undefined;
  if (!Array.isArray(content)) {
    return "";
  }
  return content
    .map((block) => (isRecord(block) && typeof block["text"] === "string" ? block["text"] : ""))
    .join("");
}

function toolCallOfStep(step: unknown): ParsedToolCall | undefined {
  if (!isRecord(step) || step["type"] !== "function_call") {
    return undefined;
  }
  const name = step["name"];
  if (typeof name !== "string" || name.length === 0) {
    return undefined;
  }
  const id = step["id"];
  const args = step["arguments"];
  return {
    id: typeof id === "string" ? id : "",
    name,
    arguments: isRecord(args) ? args : {},
  };
}

/** Read the interaction the CLI printed: model text, thoughts, function calls and usage. */
export function readInteraction(value: unknown): InteractionOutput {
  const output: InteractionOutput = {
    text: "",
    thinking: "",
    toolCalls: [],
    usage: usageOf(interactionOf(value)?.["usage"]),
  };
  for (const step of stepsOf(value)) {
    const type = isRecord(step) ? step["type"] : undefined;
    if (type === "function_call") {
      const call = toolCallOfStep(step);
      if (call) {
        output.toolCalls.push(call);
      }
      continue;
    }
    if (type === "model_output") {
      output.text += textOfStep(step);
      continue;
    }
    if (type === "thought") {
      output.thinking += textOfStep(step);
    }
  }
  return output;
}

function toolCallId(id: string, name: string, index: number): string {
  const cleaned = id.replace(/[^\w-]/gu, "_");
  return cleaned.length > 0 ? cleaned : `call_${String(index)}_${name}`;
}

function initialMessage(model: Model<Api>): AssistantMessage {
  return {
    role: "assistant",
    content: [],
    api: model.api,
    provider: model.provider,
    model: model.id,
    usage: emptyUsage(),
    stopReason: "stop",
    timestamp: Date.now(),
  };
}

type Stream = ReturnType<typeof createAssistantMessageEventStream>;
type StreamPush = Stream["push"];

function pushThinking(text: string, message: AssistantMessage, push: StreamPush): void {
  if (text.length === 0) {
    return;
  }
  const block: ThinkingContent = { type: "thinking", thinking: text };
  const contentIndex = message.content.push(block) - 1;
  push({ type: "thinking_start", contentIndex, partial: message });
  push({ type: "thinking_delta", contentIndex, delta: text, partial: message });
  push({ type: "thinking_end", contentIndex, content: text, partial: message });
}

function pushText(text: string, message: AssistantMessage, push: StreamPush): void {
  if (text.length === 0) {
    return;
  }
  const block: TextContent = { type: "text", text };
  const contentIndex = message.content.push(block) - 1;
  push({ type: "text_start", contentIndex, partial: message });
  push({ type: "text_delta", contentIndex, delta: text, partial: message });
  push({ type: "text_end", contentIndex, content: text, partial: message });
}

function pushToolCall(
  parsed: ParsedToolCall,
  index: number,
  message: AssistantMessage,
  push: StreamPush,
): void {
  const call: ToolCall = {
    type: "toolCall",
    id: toolCallId(parsed.id, parsed.name, index),
    name: parsed.name,
    arguments: parsed.arguments,
  };
  const contentIndex = message.content.push(call) - 1;
  push({ type: "toolcall_start", contentIndex, partial: message });
  push({
    type: "toolcall_delta",
    contentIndex,
    delta: JSON.stringify(call.arguments),
    partial: message,
  });
  push({ type: "toolcall_end", contentIndex, toolCall: call, partial: message });
}

async function complete(
  model: Model<Api>,
  context: TranscriptContext,
  options: GeminiCliStreamOptions,
  message: AssistantMessage,
  push: StreamPush,
): Promise<void> {
  const binary = options.binary ?? findCli();
  if (!binary) {
    throw new Error(INSTALL_HINT);
  }
  const body = buildBody({ model: model.id, messages: context.messages });
  const run = options.run ?? runCli;
  const runOptions: RunCliOptions = options.signal ? { signal: options.signal } : {};
  const { stdout } = await run(
    binary,
    ["generate", "--body", JSON.stringify(body), "--stream=false", "--output-format", "json"],
    runOptions,
  );
  const output = readInteraction(parseJson(stdout));
  pushThinking(output.thinking, message, push);
  pushText(output.text, message, push);
  output.toolCalls.forEach((call, index) => {
    pushToolCall(call, index, message, push);
  });
  message.usage = output.usage;
  if (message.content.length === 0) {
    throw new Error("gemini-api returned neither text nor a function call");
  }
  message.stopReason = output.toolCalls.length > 0 ? "toolUse" : "stop";
  push({ type: "done", reason: message.stopReason, message });
}

/** Provider entry point: one `gemini-api generate` child process per completion. */
export function streamAntigravity(
  model: Model<Api>,
  context: TranscriptContext,
  options: GeminiCliStreamOptions = {},
): Stream {
  const stream = createAssistantMessageEventStream();
  const push: StreamPush = (event) => {
    stream.push(event);
  };
  const message = initialMessage(model);
  void (async () => {
    try {
      push({ type: "start", partial: message });
      await complete(model, context, options, message, push);
    } catch (error) {
      message.stopReason = "error";
      message.errorMessage = error instanceof Error ? error.message : String(error);
      push({ type: "error", reason: "error", error: message });
    } finally {
      stream.end();
    }
  })();
  return stream;
}
