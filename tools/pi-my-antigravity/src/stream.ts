// This file runs with bun.
import { createAssistantMessageEventStream } from "@earendil-works/pi-ai";
import type {
  Api,
  AssistantMessage,
  ImageContent,
  Model,
  SimpleStreamOptions,
  TextContent,
  Tool,
  ToolCall,
  TranscriptContext,
  Usage,
} from "@earendil-works/pi-ai";

import { API_ID, CONSUMER_PROJECT, modelEnumFor } from "./client.ts";
import { ENDPOINTS, PROVIDER_ID } from "./constants.ts";
import { runtimeModelId, thinkingBudget, type ThinkingLevel } from "./models.ts";
import { sessionEnvelope } from "./session.ts";
import { currentSystemPrompt, currentTools } from "./transcript.ts";
import { postWire, type WireRequest, type WireResponse } from "./transport.ts";

/** Credential JSON handed to the provider through pi's `getApiKey`. */
export interface ResolvedAuth {
  token: string;
  projectId?: string;
}

interface GeminiPart {
  text?: string;
  thought?: boolean;
  thoughtSignature?: string;
  inlineData?: { mimeType: string; data: string };
  functionCall?: { name: string; args: Record<string, unknown> };
  functionResponse?: { name: string; response: Record<string, unknown> };
}

interface GeminiContent {
  role: "user" | "model";
  parts: GeminiPart[];
}

export interface GenerateBody {
  project: string;
  model: string;
  request: {
    contents: GeminiContent[];
    generationConfig?: Record<string, unknown>;
    tools?: Array<{ functionDeclarations: Array<Record<string, unknown>> }>;
    sessionId: string;
    labels: Record<string, string>;
  };
  requestType: "agent";
  userAgent: "antigravity";
  requestId: string;
}

export interface BuildGenerateBodyOptions {
  project: string;
  publicModelId: string;
  level: ThinkingLevel | undefined;
  systemPrompt: string | undefined;
  messages: TranscriptContext["messages"];
  tools: Tool[] | undefined;
  maxTokens: number | undefined;
  modelMaxTokens: number;
  /** pi's session id; keeps a resumed session on one trajectory. */
  sessionId?: string;
  /** `model` field of the catalog entry, sent as the `model_enum` label. */
  modelEnum?: string;
}

export interface StreamChunk {
  text?: string;
  thought?: string;
  toolCall?: { name: string; args: Record<string, unknown>; thoughtSignature?: string };
  signature?: string;
  usage?: { input?: number; output?: number; reasoning?: number };
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
  thinkingIndex: number;
  toolCalls: ToolCall[];
}

interface ParsedEnvelope {
  response?: {
    candidates?: Array<{
      content?: {
        parts?: Array<{
          text?: string;
          thought?: boolean;
          thoughtSignature?: string;
          functionCall?: { name?: string; args?: unknown };
        }>;
      };
      finishReason?: string;
    }>;
    usageMetadata?: {
      promptTokenCount?: number;
      candidatesTokenCount?: number;
      thoughtsTokenCount?: number;
    };
    responseId?: string;
  };
}

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
const SSE_DATA_PREFIX_LENGTH = 5;

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
  if (value.startsWith("{")) {
    const parsed = JSON.parse(value) as { token?: unknown; projectId?: unknown };
    if (typeof parsed.token !== "string" || !parsed.token) {
      throw new Error(
        "Antigravity credential is missing an access token; run /login my-antigravity again.",
      );
    }
    return {
      token: parsed.token,
      ...(typeof parsed.projectId === "string" && parsed.projectId
        ? { projectId: parsed.projectId }
        : {}),
    };
  }
  return { token: value };
}

const imagePart = (image: ImageContent): GeminiPart => ({
  inlineData: { mimeType: image.mimeType || "image/png", data: image.data },
});

function textOf(message: { content: string | Array<TextContent | ImageContent> }): GeminiPart[] {
  if (typeof message.content === "string") {
    return [{ text: message.content }];
  }
  return message.content.map((block) =>
    block.type === "image" ? imagePart(block) : { text: block.text },
  );
}

/**
 * Gemini's `parametersJsonSchema` accepts the JSON Schema pi's tools declare
 * (`anyOf`, `const`, nested objects). The legacy `parameters` field rejects those
 * keywords, so every model goes through the JSON Schema field.
 */
function toolDeclarations(tools: Tool[] | undefined): GenerateBody["request"]["tools"] {
  if (!tools?.length) {
    return undefined;
  }
  return [
    {
      functionDeclarations: tools.map((tool) => ({
        name: tool.name,
        description: tool.description,
        parametersJsonSchema: tool.parameters as unknown as Record<string, unknown>,
      })),
    },
  ];
}

function appendAssistantTurn(
  contents: GeminiContent[],
  blocks: Array<{
    type: string;
    text?: string;
    name?: string;
    arguments?: unknown;
    thoughtSignature?: string;
  }>,
): void {
  const parts: GeminiPart[] = [];
  for (const block of blocks) {
    if (block.type === "text" && block.text?.trim()) {
      parts.push({ text: block.text });
    }
    if (block.type === "toolCall" && block.name) {
      parts.push({
        functionCall: {
          name: block.name,
          args: (block.arguments ?? {}) as Record<string, unknown>,
        },
        ...(block.thoughtSignature ? { thoughtSignature: block.thoughtSignature } : {}),
      });
    }
  }
  if (parts.length > 0) {
    contents.push({ role: "model", parts });
  }
}

function appendToolResultTurn(
  contents: GeminiContent[],
  message: { toolName: string; isError: boolean; content: Array<TextContent | ImageContent> },
): void {
  const text = message.content
    .map((block) => (block.type === "text" ? block.text : "[image]"))
    .join("\n");
  const part: GeminiPart = {
    functionResponse: {
      name: message.toolName,
      response: message.isError ? { error: text } : { output: text },
    },
  };
  const last = contents.at(-1);
  if (last?.role === "user" && last.parts.some((entry) => entry.functionResponse !== undefined)) {
    last.parts.push(part);
    return;
  }
  contents.push({ role: "user", parts: [part] });
}

/**
 * Build the Cloud Code Assist body the official CLI sends, with one deliberate
 * deviation: the system prompt travels as the leading user turn instead of
 * `request.systemInstruction`. The backend answers the agent-flagged
 * `systemInstruction` field with a generic 429 for third-party harness prompts
 * (pi-antigravity#137); the same text in `contents` is accepted, and `agy` itself
 * sends only its own prompt text there.
 */
function appendMessage(
  contents: GeminiContent[],
  message: TranscriptContext["messages"][number],
): void {
  if (message.role === "user") {
    contents.push({ role: "user", parts: textOf(message) });
  } else if (message.role === "assistant") {
    appendAssistantTurn(contents, message.content);
  } else if (message.role === "toolResult") {
    appendToolResultTurn(contents, message);
  }
}

export function buildGenerateBody(options: BuildGenerateBodyOptions): GenerateBody {
  const runtimeModel = runtimeModelId(options.publicModelId, options.level);
  const contents: GeminiContent[] = [];
  const systemText = options.systemPrompt?.trim();
  if (systemText) {
    contents.push({ role: "user", parts: [{ text: systemText }] });
  }
  for (const message of options.messages) {
    appendMessage(contents, message);
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
  const request: GenerateBody["request"] = {
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

interface RawWirePart {
  text?: string;
  thought?: boolean;
  thoughtSignature?: string;
  functionCall?: { name?: string; args?: unknown };
}

interface RawUsageMetadata {
  promptTokenCount?: number;
  candidatesTokenCount?: number;
  thoughtsTokenCount?: number;
}

function applyTextPart(chunk: StreamChunk, text: string, thought: boolean | undefined): void {
  if (thought) {
    chunk.thought = (chunk.thought ?? "") + text;
    return;
  }
  chunk.text = (chunk.text ?? "") + text;
}

function applyPart(chunk: StreamChunk, part: RawWirePart): void {
  if (part.thoughtSignature) {
    chunk.signature = part.thoughtSignature;
  }
  if (part.functionCall?.name) {
    chunk.toolCall = {
      name: part.functionCall.name,
      args: (part.functionCall.args ?? {}) as Record<string, unknown>,
      ...(part.thoughtSignature ? { thoughtSignature: part.thoughtSignature } : {}),
    };
    return;
  }
  if (typeof part.text === "string" && part.text) {
    applyTextPart(chunk, part.text, part.thought);
  }
}

const parseUsage = (usage: RawUsageMetadata): NonNullable<StreamChunk["usage"]> => ({
  ...(typeof usage.promptTokenCount === "number" ? { input: usage.promptTokenCount } : {}),
  ...(typeof usage.candidatesTokenCount === "number" ? { output: usage.candidatesTokenCount } : {}),
  ...(typeof usage.thoughtsTokenCount === "number" ? { reasoning: usage.thoughtsTokenCount } : {}),
});

function applyCandidate(
  chunk: StreamChunk,
  candidate: NonNullable<NonNullable<ParsedEnvelope["response"]>["candidates"]>[number] | undefined,
): void {
  if (!candidate) {
    return;
  }
  for (const part of candidate.content?.parts ?? []) {
    applyPart(chunk, part);
  }
  if (candidate.finishReason) {
    chunk.finishReason = candidate.finishReason;
  }
}

/** Parse one `data:` payload from the CLI's SSE stream (`{"response":{...}}`). */
export function parseChunk(payload: string): StreamChunk | undefined {
  let parsed: ParsedEnvelope;
  try {
    parsed = JSON.parse(payload) as ParsedEnvelope;
  } catch {
    return undefined;
  }
  const response = parsed.response ?? (parsed as ParsedEnvelope["response"]);
  if (!response) {
    return undefined;
  }

  const chunk: StreamChunk = {};
  applyCandidate(chunk, response.candidates?.[0]);
  if (response.responseId) {
    chunk.responseId = response.responseId;
  }
  if (response.usageMetadata) {
    chunk.usage = parseUsage(response.usageMetadata);
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
    const payload = state.data.join("\n");
    state.data = [];
    const chunk = parseChunk(payload);
    if (chunk) {
      emit(chunk);
    }
  };
  for await (const value of response.body) {
    state.buffer += decoder.decode(value, { stream: true });
    const cursor = { offset: 0 };
    for (
      let newlineIndex = state.buffer.indexOf("\n", cursor.offset);
      newlineIndex >= 0;
      newlineIndex = state.buffer.indexOf("\n", cursor.offset)
    ) {
      const line = state.buffer.slice(cursor.offset, newlineIndex).replace(/\r$/, "");
      cursor.offset = newlineIndex + 1;
      if (line === "") {
        flush();
      } else if (line.startsWith("data:")) {
        state.data.push(line.slice(SSE_DATA_PREFIX_LENGTH).trim());
      }
    }
    if (cursor.offset > 0) {
      state.buffer = state.buffer.slice(cursor.offset);
    }
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
    throw new Error(lastFailure || "Antigravity returned no response");
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
): Promise<WireResponse> => sendAttempt(0, token, JSON.stringify(body), signal, send, "");

type StreamPush = ReturnType<typeof createAssistantMessageEventStream>["push"];

function applyUsage(usage: NonNullable<StreamChunk["usage"]>, message: AssistantMessage): void {
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
  if (state.thinkingIndex < 0) {
    state.thinkingIndex = message.content.push({ type: "thinking", thinking: "" }) - 1;
    emit({ type: "thinking_start", contentIndex: state.thinkingIndex, partial: message });
  }
  const block = message.content[state.thinkingIndex] as { thinking: string };
  block.thinking += thought;
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
  if (state.textIndex < 0) {
    state.textIndex = message.content.push({ type: "text", text: "" }) - 1;
    emit({ type: "text_start", contentIndex: state.textIndex, partial: message });
  }
  const block = message.content[state.textIndex] as { text: string };
  block.text += text;
  emit({ type: "text_delta", contentIndex: state.textIndex, delta: text, partial: message });
}

function emitToolCall(
  toolCall: NonNullable<StreamChunk["toolCall"]>,
  message: AssistantMessage,
  state: StreamEmitState,
  emit: StreamPush,
): void {
  const call: ToolCall = {
    type: "toolCall",
    id: randomId(),
    name: toolCall.name,
    arguments: toolCall.args as ToolCall["arguments"],
    ...(toolCall.thoughtSignature ? { thoughtSignature: toolCall.thoughtSignature } : {}),
  };
  state.toolCalls.push(call);
  const contentIndex = message.content.push(call) - 1;
  emit({ type: "toolcall_start", contentIndex, partial: message });
  emit({
    type: "toolcall_delta",
    contentIndex,
    delta: JSON.stringify(call.arguments),
    partial: message,
  });
  emit({ type: "toolcall_end", contentIndex, toolCall: call, partial: message });
}

function applyMetadata(
  chunk: StreamChunk,
  message: AssistantMessage,
  state: StreamEmitState,
): void {
  if (chunk.signature) {
    const lastCall = state.toolCalls.at(-1);
    if (lastCall && !lastCall.thoughtSignature) {
      lastCall.thoughtSignature = chunk.signature;
    }
  }
  if (chunk.responseId) {
    message.responseId = chunk.responseId;
  }
  if (chunk.finishReason) {
    message.rawStopReason = chunk.finishReason;
    if (chunk.finishReason === "MAX_TOKENS") {
      message.stopReason = "length";
    }
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
  if (emitState.thinkingIndex >= 0) {
    const block = message.content[emitState.thinkingIndex] as { thinking: string };
    emit({
      type: "thinking_end",
      contentIndex: emitState.thinkingIndex,
      content: block.thinking,
      partial: message,
    });
  }
  if (emitState.textIndex >= 0) {
    const block = message.content[emitState.textIndex] as { text: string };
    emit({
      type: "text_end",
      contentIndex: emitState.textIndex,
      content: block.text,
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
  const level = options.reasoning as ThinkingLevel | undefined;
  const modelEnum = modelEnumFor(runtimeModelId(model.id, level));
  const body = buildGenerateBody({
    project: auth.projectId ?? CONSUMER_PROJECT,
    publicModelId: model.id,
    level,
    systemPrompt: currentSystemPrompt(context.messages),
    messages: context.messages,
    tools: currentTools(context.messages),
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
  const emitState: StreamEmitState = { textIndex: -1, thinkingIndex: -1, toolCalls: [] };
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
