import { CLAUDEX_BACKGROUND_BASH_GUIDANCE } from "@kkkaoru/pi-tmux-timeout-extension/policy";
import type {
  Api,
  AssistantMessage,
  Context,
  ImageContent,
  Message,
  Model,
  TextContent,
  ThinkingContent,
  Tool,
  ToolCall,
  ToolResultMessage,
  Usage,
  UserMessage,
} from "@earendil-works/pi-ai";
import { Unsafe } from "typebox";
import { GatewayError } from "./errors.ts";
import { isRecord, type JsonRecord, type StreamRequestMessage } from "./protocol.ts";

interface ConversionState {
  model: Model<Api>;
  requestId: string;
  timestamp: number;
  toolNames: Map<string, string>;
  systemValues: unknown[];
}

function requiredRecord(value: unknown, label: string, requestId: string): JsonRecord {
  if (!isRecord(value)) {
    throw new GatewayError(`Anthropic ${label} must be an object`, requestId);
  }
  return value;
}

function requiredText(value: JsonRecord, key: string, requestId: string): string {
  const result = value[key];
  if (typeof result !== "string") {
    throw new GatewayError(`Anthropic field ${key} must be a string`, requestId);
  }
  return result;
}

function optionalText(value: JsonRecord, key: string, requestId: string): string | undefined {
  const result = value[key];
  if (result === undefined) {
    return undefined;
  }
  if (typeof result !== "string") {
    throw new GatewayError(`Anthropic field ${key} must be a string`, requestId);
  }
  return result;
}

const ADAPTER_THINKING_SIGNATURE_PREFIX = "claudex_";
const SKILL_CALL_SHAPE =
  'Required call shape: {"skill":"<exact available skill name>","args":"<optional arguments>"}. Never call Skill with an empty object; the skill field is mandatory.';

function isAdapterThinkingSignature(signature: string): boolean {
  return signature.startsWith(ADAPTER_THINKING_SIGNATURE_PREFIX) && !signature.startsWith("{");
}

function providerThinkingSignature(signature: string | undefined): string | undefined {
  if (signature === undefined || isAdapterThinkingSignature(signature)) {
    return undefined;
  }
  return signature;
}

function emptyUsage(): Usage {
  return {
    input: 0,
    output: 0,
    cacheRead: 0,
    cacheWrite: 0,
    totalTokens: 0,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
  };
}

function createUserMessage(content: UserMessage["content"], timestamp: number): UserMessage {
  return { role: "user", content, timestamp };
}

function parseImage(block: JsonRecord, requestId: string): ImageContent {
  const source = requiredRecord(block["source"], "image source", requestId);
  if (source["type"] !== "base64") {
    throw new GatewayError("Only base64 Anthropic images are supported", requestId);
  }
  return {
    type: "image",
    data: requiredText(source, "data", requestId),
    mimeType: requiredText(source, "media_type", requestId),
  };
}

function parseToolResultContent(value: unknown, requestId: string): (TextContent | ImageContent)[] {
  if (typeof value === "string") {
    return [{ type: "text", text: value }];
  }
  if (!Array.isArray(value)) {
    throw new GatewayError("Anthropic tool result content must be string or array", requestId);
  }
  return value.map((part, index) => {
    const record = requiredRecord(part, `tool result block ${index}`, requestId);
    if (record["type"] === "text") {
      return { type: "text", text: requiredText(record, "text", requestId) };
    }
    if (record["type"] === "image") {
      return parseImage(record, requestId);
    }
    throw new GatewayError(`Anthropic tool result block ${index} has unsupported type`, requestId);
  });
}

function parseToolResult(block: JsonRecord, state: ConversionState): ToolResultMessage {
  const toolCallId = requiredText(block, "tool_use_id", state.requestId);
  const toolName = state.toolNames.get(toolCallId);
  if (toolName === undefined) {
    throw new GatewayError(
      `Tool result references unknown tool call ${toolCallId}`,
      state.requestId,
    );
  }
  return {
    role: "toolResult",
    toolCallId,
    toolName,
    content: parseToolResultContent(block["content"], state.requestId),
    isError: block["is_error"] === true,
    timestamp: state.timestamp,
  };
}

function parseAssistantBlock(
  value: unknown,
  index: number,
  state: ConversionState,
): TextContent | ThinkingContent | ToolCall {
  const block = requiredRecord(value, `assistant block ${index}`, state.requestId);
  if (block["type"] === "text") {
    return { type: "text", text: requiredText(block, "text", state.requestId) };
  }
  if (block["type"] === "thinking") {
    const signature = providerThinkingSignature(optionalText(block, "signature", state.requestId));
    return {
      type: "thinking",
      thinking: requiredText(block, "thinking", state.requestId),
      ...(signature === undefined ? {} : { thinkingSignature: signature }),
    };
  }
  if (block["type"] === "redacted_thinking") {
    return {
      type: "thinking",
      thinking: "",
      thinkingSignature: requiredText(block, "data", state.requestId),
      redacted: true,
    };
  }
  if (block["type"] !== "tool_use") {
    throw new GatewayError(
      `Anthropic assistant block ${index} has unsupported type`,
      state.requestId,
    );
  }
  const id = requiredText(block, "id", state.requestId);
  const name = requiredText(block, "name", state.requestId);
  const input = requiredRecord(block["input"], `tool input ${id}`, state.requestId);
  state.toolNames.set(id, name);
  return { type: "toolCall", id, name, arguments: input };
}

function parseAssistantMessage(
  content: unknown,
  index: number,
  state: ConversionState,
): AssistantMessage {
  const blocks = typeof content === "string" ? [{ type: "text", text: content }] : content;
  if (!Array.isArray(blocks)) {
    throw new GatewayError(
      `Anthropic assistant message ${index} content must be string or array`,
      state.requestId,
    );
  }
  const converted = blocks.map((block, blockIndex) =>
    parseAssistantBlock(block, blockIndex, state),
  );
  return {
    role: "assistant",
    content: converted,
    api: state.model.api,
    provider: state.model.provider,
    model: state.model.id,
    usage: emptyUsage(),
    stopReason: converted.some((block) => block.type === "toolCall") ? "toolUse" : "stop",
    timestamp: state.timestamp + index,
  };
}

function parseUserMessage(content: unknown, index: number, state: ConversionState): Message[] {
  if (typeof content === "string") {
    return [createUserMessage(content, state.timestamp + index)];
  }
  if (!Array.isArray(content)) {
    throw new GatewayError(
      `Anthropic user message ${index} content must be string or array`,
      state.requestId,
    );
  }
  const messages: Message[] = [];
  let userParts: (TextContent | ImageContent)[] = [];
  const flushUserParts = (): void => {
    if (userParts.length > 0) {
      messages.push(createUserMessage(userParts, state.timestamp + index));
      userParts = [];
    }
  };
  for (const [blockIndex, block] of content.entries()) {
    const record = requiredRecord(block, `user block ${blockIndex}`, state.requestId);
    if (record["type"] === "text") {
      userParts.push({ type: "text", text: requiredText(record, "text", state.requestId) });
    } else if (record["type"] === "image") {
      userParts.push(parseImage(record, state.requestId));
    } else if (record["type"] === "tool_result") {
      flushUserParts();
      messages.push(parseToolResult(record, state));
    } else {
      throw new GatewayError(
        `Anthropic user block ${blockIndex} has unsupported type`,
        state.requestId,
      );
    }
  }
  flushUserParts();
  return messages;
}

function parseMessage(value: unknown, index: number, state: ConversionState): Message[] {
  const message = requiredRecord(value, `message ${index}`, state.requestId);
  const { role } = message;
  if (role === "user") {
    return parseUserMessage(message["content"], index, state);
  }
  if (role === "assistant") {
    return [parseAssistantMessage(message["content"], index, state)];
  }
  if (role === "system") {
    state.systemValues.push(message["content"]);
    return [];
  }
  throw new GatewayError(`Anthropic message ${index} has unsupported role`, state.requestId);
}

function parseSystemPrompt(value: unknown, requestId: string): string | undefined {
  if (value === undefined || value === null || value === "") {
    return undefined;
  }
  if (typeof value === "string") {
    return value;
  }
  if (!Array.isArray(value)) {
    throw new GatewayError("Anthropic system must be a string or text-block array", requestId);
  }
  return value
    .map((block, index) => {
      const record = requiredRecord(block, `system block ${index}`, requestId);
      if (record["type"] !== "text") {
        throw new GatewayError(`Anthropic system block ${index} must contain text`, requestId);
      }
      return requiredText(record, "text", requestId);
    })
    .join("\n\n");
}

function normalizeToolSchema(value: unknown): JsonRecord {
  // Keep the Pi tool contract object-shaped.
  // An upstream tool may omit or provide an invalid Anthropic schema.
  return isRecord(value) ? value : { type: "object" };
}

function appendedDescription(description: string, guidance: string): string {
  return description === "" ? guidance : `${description}\n\n${guidance}`;
}

function toolDescription(name: string, value: unknown): string {
  const description = typeof value === "string" ? value : "";
  if (name === "Skill") {
    return appendedDescription(description, SKILL_CALL_SHAPE);
  }
  return name === "Bash"
    ? appendedDescription(description, CLAUDEX_BACKGROUND_BASH_GUIDANCE)
    : description;
}

function parseTool(value: unknown, index: number, requestId: string): Tool {
  const tool = requiredRecord(value, `tool ${index}`, requestId);
  const name = requiredText(tool, "name", requestId);
  const schema = normalizeToolSchema(tool["input_schema"]);
  return {
    name,
    description: toolDescription(name, tool["description"]),
    parameters: Unsafe(schema),
  };
}

export function toPiContext(request: StreamRequestMessage, model: Model<Api>): Context {
  const state: ConversionState = {
    model,
    requestId: request.id,
    timestamp: Date.now(),
    toolNames: new Map(),
    systemValues: [],
  };
  const messages = request.messages.flatMap((message, index) =>
    parseMessage(message, index, state),
  );
  const combinedSystemPrompt = [request.system, ...state.systemValues]
    .map((value) => parseSystemPrompt(value, request.id))
    .filter((value): value is string => value !== undefined)
    .join("\n\n");
  const systemPrompt = combinedSystemPrompt === "" ? undefined : combinedSystemPrompt;
  const tools = request.tools.map((tool, index) => parseTool(tool, index, request.id));
  return {
    ...(systemPrompt === undefined ? {} : { systemPrompt }),
    messages,
    ...(tools.length === 0 ? {} : { tools }),
  };
}
