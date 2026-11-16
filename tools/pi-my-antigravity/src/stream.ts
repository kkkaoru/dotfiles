// This file runs with bun.
import { createAssistantMessageEventStream } from "@earendil-works/pi-ai";
import type {
  Api,
  AssistantMessage,
  ImageContent,
  JsonObject,
  Model,
  SimpleStreamOptions,
  TextContent,
  ThinkingContent,
  Tool,
  ToolCall,
  TranscriptContext,
  Usage,
} from "@earendil-works/pi-ai";

import { API_ID, CONSUMER_PROJECT, modelEnumFor } from "./client.ts";
import { ENDPOINTS, PROVIDER_ID } from "./constants.ts";
import { isRecord, nonEmptyString, parseJson, stringifyJson, tryParseJson } from "./json.ts";
import { runtimeModelId, thinkingBudget, type ModelThinkingLevel } from "./models.ts";
import { sessionEnvelope } from "./session.ts";
import { replayTranscript } from "./transcript.ts";
import { postWire, type WireRequest, type WireResponse } from "./transport.ts";

/** Credential JSON handed to the provider through pi's `getApiKey`. */
export interface ResolvedAuth {
  token: string;
  projectId?: string;
}

interface GeminiInlineData {
  mimeType: string;
  data: string;
}

interface GeminiFunctionCall {
  name: string;
  args: JsonObject;
  id?: string;
}

interface GeminiFunctionResponse {
  name: string;
  response: Record<string, string>;
  id?: string;
}

interface GeminiPart {
  text?: string;
  thought?: boolean;
  thoughtSignature?: string;
  inlineData?: GeminiInlineData;
  functionCall?: GeminiFunctionCall;
  functionResponse?: GeminiFunctionResponse;
}

interface GeminiContent {
  role: "user" | "model";
  parts: GeminiPart[];
}

export interface FunctionDeclaration {
  name: string;
  description: string;
  parametersJsonSchema: Tool["parameters"];
}

export interface ToolDeclarationGroup {
  functionDeclarations: FunctionDeclaration[];
}

export interface GenerateRequestPayload {
  contents: GeminiContent[];
  generationConfig?: Record<string, unknown>;
  tools?: ToolDeclarationGroup[];
  sessionId: string;
  labels: Record<string, string>;
}

export interface GenerateBody {
  project: string;
  model: string;
  request: GenerateRequestPayload;
  requestType: "agent";
  userAgent: "antigravity";
  requestId: string;
}

export interface BuildGenerateBodyOptions {
  project: string;
  publicModelId: string;
  level: ModelThinkingLevel | undefined;
  systemPrompt: string | undefined;
  messages: TranscriptContext["messages"];
  tools: Tool[] | undefined;
  maxTokens: number | undefined;
  modelMaxTokens: number;
  /** pi's session id; keeps a resumed session on one trajectory. */
  sessionId?: string;
  /** `model` field of the catalog entry, sent as the `model_enum` label. */
  modelEnum?: string;
  /** Pre-resolved runtime id to avoid recomputing it in `runStream`. */
  runtimeModel?: string;
}

export interface StreamToolCall {
  id?: string;
  name: string;
  args: JsonObject;
  thoughtSignature?: string;
}

export interface StreamUsage {
  input?: number;
  output?: number;
  reasoning?: number;
}

export interface StreamChunk {
  text?: string;
  thought?: string;
  toolCall?: StreamToolCall;
  signature?: string;
  usage?: StreamUsage;
  finishReason?: string;
  responseId?: string;
}

export interface AntigravityStreamOptions extends SimpleStreamOptions {
  send?: (request: WireRequest) => Promise<WireResponse>;
}

interface StreamReadState {
  buffer: string;
  data: string[];
}

interface StreamEmitState {
  textIndex: number;
  textBlock: TextContent | undefined;
  thinkingIndex: number;
  thinkingBlock: ThinkingContent | undefined;
  toolCalls: ToolCall[];
}

interface ToolResultInput {
  toolCallId?: string;
  toolName: string;
  isError: boolean;
  content: Array<TextContent | ImageContent>;
}

type StreamPush = ReturnType<typeof createAssistantMessageEventStream>["push"];

const HTTP_OK_MIN = 200;
const HTTP_OK_MAX = 300;
const HTTP_UNAUTHORIZED = 401;
const HTTP_FORBIDDEN = 403;
const HTTP_NOT_FOUND = 404;
const HTTP_RATE_LIMIT = 429;
const HTTP_SERVER_ERROR = 500;
const HTTP_SERVICE_UNAVAILABLE = 503;
const DEFAULT_MAX_TOKENS = 65536;
const MAX_ERROR_SLICE = 300;
const MAX_RATE_LIMIT_SLICE = 200;
const SSE_DATA_PREFIX = "data:";
const SSE_DATA_PREFIX_LENGTH = 5;
const CR_CHAR_CODE = 13;
const SPACE_CHAR_CODE = 32;

const RETRYABLE_STATUSES: ReadonlySet<number> = new Set([
  HTTP_NOT_FOUND,
  HTTP_SERVER_ERROR,
  HTTP_SERVICE_UNAVAILABLE,
]);

const randomId = (): string => crypto.randomUUID();

export function parseAuthValue(raw: string | undefined): ResolvedAuth {
  const value = (raw ?? "").trim();
  if (!value) {
    throw new Error("No Antigravity credential. Run /login my-antigravity first.");
  }
  if (!value.startsWith("{")) {
    return { token: value };
  }
  const parsed = parseJson(value);
  if (!isRecord(parsed) || typeof parsed["token"] !== "string" || !parsed["token"]) {
    throw new Error(
      "Antigravity credential is missing an access token; run /login my-antigravity again.",
    );
  }
  return {
    token: parsed["token"],
    ...(typeof parsed["projectId"] === "string" && parsed["projectId"]
      ? { projectId: parsed["projectId"] }
      : {}),
  };
}

const imagePart = (image: ImageContent): GeminiPart => ({
  inlineData: { mimeType: image.mimeType || "image/png", data: image.data },
});

const textOf = (message: { content: string | Array<TextContent | ImageContent> }): GeminiPart[] =>
  typeof message.content === "string"
    ? [{ text: message.content }]
    : message.content.map((block) =>
        block.type === "image" ? imagePart(block) : { text: block.text },
      );

const toFunctionDeclaration = (tool: Tool): FunctionDeclaration => ({
  name: tool.name,
  description: tool.description,
  parametersJsonSchema: tool.parameters,
});

interface ToolCache {
  tools: Tool[] | undefined;
  groups: ToolDeclarationGroup[] | undefined;
}

const TOOL_CACHE: ToolCache = { tools: undefined, groups: undefined };

function sameToolList(left: readonly Tool[] | undefined, right: readonly Tool[]): boolean {
  if (!left || left.length !== right.length) {
    return false;
  }
  for (let index = 0; index < left.length; index += 1) {
    if (left[index] !== right[index]) {
      return false;
    }
  }
  return true;
}

/**
 * Gemini's `parametersJsonSchema` accepts the JSON Schema pi's tools declare
 * (`anyOf`, `const`, nested objects). The legacy `parameters` field rejects those
 * keywords, so every model goes through the JSON Schema field.
 */
function toolDeclarations(tools: Tool[] | undefined): ToolDeclarationGroup[] | undefined {
  if (!tools?.length) {
    return undefined;
  }
  if (sameToolList(TOOL_CACHE.tools, tools)) {
    return TOOL_CACHE.groups;
  }
  const groups: ToolDeclarationGroup[] = [
    { functionDeclarations: tools.map(toFunctionDeclaration) },
  ];
  TOOL_CACHE.tools = tools;
  TOOL_CACHE.groups = groups;
  return groups;
}

const MAX_TOOL_ID_LENGTH = 64;

const needsToolCallId = (modelId: string): boolean =>
  modelId.startsWith("claude-") || modelId.startsWith("gpt-oss-");

function sanitizeToolCallId(rawId: string | undefined, fallbackName: string): string {
  const cleaned = (rawId ?? "").replace(/[^a-zA-Z0-9_-]/g, "_").slice(0, MAX_TOOL_ID_LENGTH);
  return cleaned || `${fallbackName.replace(/[^a-zA-Z0-9_-]/g, "_")}_${randomId().slice(0, 8)}`;
}

function toAssistantPart(
  block: TextContent | ThinkingContent | ToolCall,
  includeToolId: boolean,
): GeminiPart | undefined {
  if (block.type === "text" && block.text.trim()) {
    return { text: block.text };
  }
  if (block.type === "thinking" && block.thinking.trim() && block.thinkingSignature) {
    return {
      thought: true,
      text: block.thinking,
      thoughtSignature: block.thinkingSignature,
    };
  }
  if (block.type !== "toolCall") {
    return undefined;
  }
  return {
    functionCall: {
      name: block.name,
      args: block.arguments,
      ...(includeToolId ? { id: sanitizeToolCallId(block.id, block.name) } : {}),
    },
    ...(block.thoughtSignature ? { thoughtSignature: block.thoughtSignature } : {}),
  };
}

function appendAssistantTurn(
  contents: GeminiContent[],
  blocks: Array<TextContent | ThinkingContent | ToolCall>,
  includeToolId: boolean,
): void {
  const parts: GeminiPart[] = [];
  for (const block of blocks) {
    const part = toAssistantPart(block, includeToolId);
    if (part) {
      parts.push(part);
    }
  }
  if (parts.length > 0) {
    contents.push({ role: "model", parts });
  }
}

function toolResultText(content: Array<TextContent | ImageContent>): string {
  if (content.length === 1 && content[0]?.type === "text") {
    return content[0].text;
  }
  const parts: string[] = [];
  for (const block of content) {
    parts.push(block.type === "text" ? block.text : "[image]");
  }
  return parts.join("\n");
}

function appendToolResultTurn(
  contents: GeminiContent[],
  message: ToolResultInput,
  includeToolId: boolean,
): void {
  const text = toolResultText(message.content);
  const part: GeminiPart = {
    functionResponse: {
      name: message.toolName,
      response: message.isError ? { error: text } : { output: text },
      ...(includeToolId ? { id: sanitizeToolCallId(message.toolCallId, message.toolName) } : {}),
    },
  };
  const last = contents.at(-1);
  if (last?.role === "user" && last.parts[0]?.functionResponse !== undefined) {
    last.parts.push(part);
    return;
  }
  contents.push({ role: "user", parts: [part] });
}

function appendMessage(
  contents: GeminiContent[],
  message: TranscriptContext["messages"][number],
  includeToolId: boolean,
): void {
  if (message.role === "user") {
    contents.push({ role: "user", parts: textOf(message) });
  } else if (message.role === "assistant") {
    appendAssistantTurn(contents, message.content, includeToolId);
  } else if (message.role === "toolResult") {
    appendToolResultTurn(contents, message, includeToolId);
  }
}

/**
 * Build the Cloud Code Assist body the official CLI sends, with one deliberate
 * deviation: the system prompt travels as the leading user turn instead of
 * `request.systemInstruction`. The backend answers the agent-flagged
 * `systemInstruction` field with a generic 429 for third-party harness prompts
 * (pi-antigravity#137); the same text in `contents` is accepted, and `agy` itself
 * sends only its own prompt text there.
 */
export function buildGenerateBody(options: BuildGenerateBodyOptions): GenerateBody {
  const runtimeModel = options.runtimeModel ?? runtimeModelId(options.publicModelId, options.level);
  const includeToolId = needsToolCallId(options.publicModelId) || needsToolCallId(runtimeModel);
  const contents: GeminiContent[] = [];
  const systemText = options.systemPrompt?.trim();
  if (systemText) {
    contents.push({ role: "user", parts: [{ text: systemText }] });
  }
  for (const message of options.messages) {
    appendMessage(contents, message, includeToolId);
  }

  const maxTokens = Math.min(options.maxTokens ?? options.modelMaxTokens, options.modelMaxTokens);
  const generationConfig: Record<string, unknown> = {
    thinkingConfig: {
      includeThoughts: true,
      thinkingBudget: thinkingBudget(options.level),
    },
    maxOutputTokens: maxTokens,
  };

  const envelope = sessionEnvelope({
    ...(options.sessionId ? { sessionId: options.sessionId } : {}),
    messages: options.messages,
    step: contents.length,
    isClaude: options.publicModelId.startsWith("claude-"),
    isNonGemini: !options.publicModelId.startsWith("gemini-"),
    ...(options.modelEnum ? { modelEnum: options.modelEnum } : {}),
  });
  const request: GenerateRequestPayload = {
    contents,
    generationConfig,
    sessionId: envelope.sessionId,
    labels: envelope.labels,
  };
  const tools = toolDeclarations(options.tools);
  if (tools) {
    request.tools = tools;
  }

  return {
    project: options.project,
    model: runtimeModel,
    request,
    requestType: "agent",
    userAgent: "antigravity",
    requestId: envelope.requestId,
  };
}

function applyTextPart(chunk: StreamChunk, text: string, thought: unknown): void {
  if (thought === true) {
    chunk.thought = (chunk.thought ?? "") + text;
    return;
  }
  chunk.text = (chunk.text ?? "") + text;
}

function applyPart(chunk: StreamChunk, part: unknown): void {
  if (!isRecord(part)) {
    return;
  }
  const thoughtSignature = nonEmptyString(part["thoughtSignature"]);
  if (thoughtSignature) {
    chunk.signature = thoughtSignature;
  }
  const call = part["functionCall"];
  if (isRecord(call)) {
    const name = nonEmptyString(call["name"]);
    if (name) {
      const rawArgs = call["args"];
      const callId = nonEmptyString(call["id"]);
      chunk.toolCall = {
        ...(callId ? { id: callId } : {}),
        name,
        args: isRecord(rawArgs) ? rawArgs : {},
        ...(thoughtSignature ? { thoughtSignature } : {}),
      };
      return;
    }
  }
  const text = nonEmptyString(part["text"]);
  if (text) {
    applyTextPart(chunk, text, part["thought"]);
  }
}

const parseUsage = (usage: Record<string, unknown>): StreamUsage => ({
  ...(typeof usage["promptTokenCount"] === "number" ? { input: usage["promptTokenCount"] } : {}),
  ...(typeof usage["candidatesTokenCount"] === "number"
    ? { output: usage["candidatesTokenCount"] }
    : {}),
  ...(typeof usage["thoughtsTokenCount"] === "number"
    ? { reasoning: usage["thoughtsTokenCount"] }
    : {}),
});

function applyCandidate(chunk: StreamChunk, candidate: unknown): void {
  if (!isRecord(candidate)) {
    return;
  }
  const content = candidate["content"];
  if (isRecord(content) && Array.isArray(content["parts"])) {
    for (const part of content["parts"]) {
      applyPart(chunk, part);
    }
  }
  if (typeof candidate["finishReason"] === "string" && candidate["finishReason"]) {
    chunk.finishReason = candidate["finishReason"];
  }
}

/** Parse one `data:` payload from the CLI's SSE stream (`{"response":{...}}`). */
export function parseChunk(payload: string): StreamChunk | undefined {
  const parsed = tryParseJson(payload);
  if (!isRecord(parsed)) {
    return undefined;
  }
  const response = isRecord(parsed["response"]) ? parsed["response"] : parsed;
  const chunk: StreamChunk = {};
  if (Array.isArray(response["candidates"])) {
    applyCandidate(chunk, response["candidates"][0]);
  }
  if (typeof response["responseId"] === "string" && response["responseId"]) {
    chunk.responseId = response["responseId"];
  }
  if (isRecord(response["usageMetadata"])) {
    chunk.usage = parseUsage(response["usageMetadata"]);
  }
  return chunk;
}

const emptyUsage = (): Usage => ({
  input: 0,
  output: 0,
  cacheRead: 0,
  cacheWrite: 0,
  totalTokens: 0,
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
});

const initialMessage = (model: Model<Api>): AssistantMessage => ({
  role: "assistant",
  content: [],
  api: API_ID,
  provider: PROVIDER_ID,
  model: model.id,
  usage: emptyUsage(),
  stopReason: "stop",
  timestamp: Date.now(),
});

function friendlyError(status: number, body: string): string {
  if (status === HTTP_UNAUTHORIZED || status === HTTP_FORBIDDEN) {
    return `Antigravity rejected the credential (${status}). Run /login my-antigravity again.`;
  }
  if (status === HTTP_RATE_LIMIT) {
    return `Antigravity rate-limited or blocked this request (429). ${body.slice(0, MAX_RATE_LIMIT_SLICE)}`;
  }
  return `Antigravity request failed (${status}). ${body.slice(0, MAX_ERROR_SLICE)}`;
}

function lineEndWithoutCr(buffer: string, start: number, newlineIndex: number): number {
  return newlineIndex > start && buffer.charCodeAt(newlineIndex - 1) === CR_CHAR_CODE
    ? newlineIndex - 1
    : newlineIndex;
}

function extractDataPayload(buffer: string, start: number, end: number): string | undefined {
  if (!buffer.startsWith(SSE_DATA_PREFIX, start)) {
    return undefined;
  }
  const offset = { start: start + SSE_DATA_PREFIX_LENGTH, end };
  while (offset.start < offset.end && buffer.charCodeAt(offset.start) <= SPACE_CHAR_CODE) {
    offset.start += 1;
  }
  while (offset.end > offset.start && buffer.charCodeAt(offset.end - 1) <= SPACE_CHAR_CODE) {
    offset.end -= 1;
  }
  return buffer.slice(offset.start, offset.end);
}

function drainLines(state: StreamReadState, flush: () => void): void {
  const cursor = { offset: 0 };
  while (true) {
    const newlineIndex = state.buffer.indexOf("\n", cursor.offset);
    if (newlineIndex < 0) {
      break;
    }
    const end = lineEndWithoutCr(state.buffer, cursor.offset, newlineIndex);
    const start = cursor.offset;
    cursor.offset = newlineIndex + 1;
    if (end === start) {
      flush();
      continue;
    }
    const data = extractDataPayload(state.buffer, start, end);
    if (data !== undefined) {
      state.data.push(data);
    }
  }
  if (cursor.offset === state.buffer.length) {
    state.buffer = "";
  } else if (cursor.offset > 0) {
    state.buffer = state.buffer.slice(cursor.offset);
  }
}

async function readStream(
  response: { body: AsyncIterable<Buffer> },
  emit: (chunk: StreamChunk) => void,
): Promise<void> {
  const decoder = new TextDecoder();
  const state: StreamReadState = { buffer: "", data: [] };
  const flush = (): void => {
    if (state.data.length === 0) {
      return;
    }
    const payload = state.data.length === 1 ? (state.data[0] ?? "") : state.data.join("\n");
    state.data.length = 0;
    const chunk = parseChunk(payload);
    if (chunk) {
      emit(chunk);
    }
  };
  for await (const value of response.body) {
    state.buffer += decoder.decode(value, { stream: true });
    drainLines(state, flush);
  }
  const tail = decoder.decode();
  if (tail || state.buffer) {
    state.buffer += `${tail}\n`;
    drainLines(state, flush);
  }
  flush();
}

async function sendAttempt(
  index: number,
  token: string,
  payload: string,
  signal: AbortSignal | undefined,
  send: (request: WireRequest) => Promise<WireResponse>,
  lastFailure: string,
): Promise<WireResponse> {
  const endpoint = ENDPOINTS[index];
  if (!endpoint) {
    throw new Error(lastFailure);
  }
  const attempt = await send({
    url: `${endpoint}/v1internal:streamGenerateContent?alt=sse`,
    token,
    body: payload,
    accept: "text/event-stream",
    ...(signal ? { signal } : {}),
  });
  if (attempt.status >= HTTP_OK_MIN && attempt.status < HTTP_OK_MAX) {
    return attempt;
  }
  const failure = friendlyError(attempt.status, await attempt.text());
  if (!RETRYABLE_STATUSES.has(attempt.status)) {
    throw new Error(failure);
  }
  return await sendAttempt(index + 1, token, payload, signal, send, failure);
}

const sendGenerateRequest = (
  token: string,
  body: GenerateBody,
  signal: AbortSignal | undefined,
  send: (request: WireRequest) => Promise<WireResponse>,
): Promise<WireResponse> => sendAttempt(0, token, stringifyJson(body), signal, send, "");

function applyUsage(usage: StreamUsage, message: AssistantMessage): void {
  message.usage.input = usage.input ?? message.usage.input;
  message.usage.output = usage.output ?? message.usage.output;
  if (usage.reasoning !== undefined) {
    message.usage.reasoning = usage.reasoning;
  }
  message.usage.totalTokens =
    message.usage.input + message.usage.output + message.usage.cacheRead + message.usage.cacheWrite;
}

function emitThought(
  thought: string,
  message: AssistantMessage,
  state: StreamEmitState,
  emit: StreamPush,
): void {
  if (!state.thinkingBlock) {
    const block: ThinkingContent = { type: "thinking", thinking: "" };
    state.thinkingBlock = block;
    state.thinkingIndex = message.content.push(block) - 1;
    emit({ type: "thinking_start", contentIndex: state.thinkingIndex, partial: message });
  }
  state.thinkingBlock.thinking += thought;
  emit({
    type: "thinking_delta",
    contentIndex: state.thinkingIndex,
    delta: thought,
    partial: message,
  });
}

function emitText(
  text: string,
  message: AssistantMessage,
  state: StreamEmitState,
  emit: StreamPush,
): void {
  if (!state.textBlock) {
    const block: TextContent = { type: "text", text: "" };
    state.textBlock = block;
    state.textIndex = message.content.push(block) - 1;
    emit({ type: "text_start", contentIndex: state.textIndex, partial: message });
  }
  state.textBlock.text += text;
  emit({ type: "text_delta", contentIndex: state.textIndex, delta: text, partial: message });
}

function emitToolCall(
  toolCall: StreamToolCall,
  message: AssistantMessage,
  state: StreamEmitState,
  emit: StreamPush,
): void {
  const call: ToolCall = {
    type: "toolCall",
    id: sanitizeToolCallId(toolCall.id, toolCall.name),
    name: toolCall.name,
    arguments: toolCall.args,
    ...(toolCall.thoughtSignature ? { thoughtSignature: toolCall.thoughtSignature } : {}),
  };
  state.toolCalls.push(call);
  const contentIndex = message.content.push(call) - 1;
  emit({ type: "toolcall_start", contentIndex, partial: message });
  emit({
    type: "toolcall_delta",
    contentIndex,
    delta: stringifyJson(call.arguments),
    partial: message,
  });
  emit({ type: "toolcall_end", contentIndex, toolCall: call, partial: message });
}

function attachLateSignature(signature: string | undefined, state: StreamEmitState): void {
  if (!signature) {
    return;
  }
  const lastCall = state.toolCalls.at(-1);
  if (lastCall && !lastCall.thoughtSignature) {
    lastCall.thoughtSignature = signature;
    return;
  }
  if (state.thinkingBlock && !state.thinkingBlock.thinkingSignature) {
    state.thinkingBlock.thinkingSignature = signature;
  }
}

function applyMetadata(
  chunk: StreamChunk,
  message: AssistantMessage,
  state: StreamEmitState,
): void {
  attachLateSignature(chunk.signature, state);
  if (chunk.responseId) {
    message.responseId = chunk.responseId;
  }
  if (!chunk.finishReason) {
    return;
  }
  message.rawStopReason = chunk.finishReason;
  if (chunk.finishReason === "MAX_TOKENS") {
    message.stopReason = "length";
  }
}

function applyChunk(
  chunk: StreamChunk,
  message: AssistantMessage,
  state: StreamEmitState,
  emit: StreamPush,
): void {
  if (chunk.usage) {
    applyUsage(chunk.usage, message);
  }
  if (chunk.thought) {
    emitThought(chunk.thought, message, state, emit);
  }
  if (chunk.text) {
    emitText(chunk.text, message, state, emit);
  }
  if (chunk.toolCall) {
    emitToolCall(chunk.toolCall, message, state, emit);
  }
  applyMetadata(chunk, message, state);
}

function finalizeStream(
  emitState: StreamEmitState,
  message: AssistantMessage,
  emit: StreamPush,
): void {
  if (emitState.thinkingBlock) {
    emit({
      type: "thinking_end",
      contentIndex: emitState.thinkingIndex,
      content: emitState.thinkingBlock.thinking,
      partial: message,
    });
  }
  if (emitState.textBlock) {
    emit({
      type: "text_end",
      contentIndex: emitState.textIndex,
      content: emitState.textBlock.text,
      partial: message,
    });
  }
  if (emitState.toolCalls.length > 0) {
    message.stopReason = "toolUse";
    emit({ type: "done", reason: "toolUse", message });
    return;
  }
  const reason = message.stopReason === "length" ? "length" : "stop";
  message.stopReason = reason;
  emit({ type: "done", reason, message });
}

async function runStream(
  model: Model<Api>,
  context: TranscriptContext,
  options: AntigravityStreamOptions,
  message: AssistantMessage,
  emit: StreamPush,
): Promise<void> {
  const auth = parseAuthValue(options.apiKey);
  const runtimeModel = runtimeModelId(model.id, options.reasoning);
  const modelEnum = modelEnumFor(runtimeModel);
  const transcript = replayTranscript(context.messages);
  const body = buildGenerateBody({
    project: auth.projectId ?? CONSUMER_PROJECT,
    publicModelId: model.id,
    level: options.reasoning,
    runtimeModel,
    systemPrompt: transcript.systemPrompt,
    messages: context.messages,
    tools: transcript.tools,
    maxTokens: options.maxTokens,
    modelMaxTokens: model.maxTokens || DEFAULT_MAX_TOKENS,
    ...(options.sessionId ? { sessionId: options.sessionId } : {}),
    ...(modelEnum ? { modelEnum } : {}),
  });

  emit({ type: "start", partial: message });
  const response = await sendGenerateRequest(
    auth.token,
    body,
    options.signal,
    options.send ?? postWire,
  );
  const emitState: StreamEmitState = {
    textIndex: -1,
    textBlock: undefined,
    thinkingIndex: -1,
    thinkingBlock: undefined,
    toolCalls: [],
  };
  await readStream(response, (chunk) => applyChunk(chunk, message, emitState, emit));
  finalizeStream(emitState, message, emit);
}

export function streamAntigravity(
  model: Model<Api>,
  context: TranscriptContext,
  options: AntigravityStreamOptions = {},
): ReturnType<typeof createAssistantMessageEventStream> {
  const stream = createAssistantMessageEventStream();
  const emit = stream.push.bind(stream);

  void (async () => {
    const message = initialMessage(model);
    try {
      await runStream(model, context, options, message, emit);
    } catch (error) {
      message.stopReason = "error";
      message.errorMessage = error instanceof Error ? error.message : String(error);
      emit({ type: "error", reason: "error", error: message });
    } finally {
      stream.end();
    }
  })();

  return stream;
}
