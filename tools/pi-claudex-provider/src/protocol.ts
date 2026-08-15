import { GatewayError } from "./errors.ts";

export const PROTOCOL_VERSION = 1;
export const CLAUDEX_ORIGIN = "claudex";
const MAX_IDENTIFIER_LENGTH = 256;
const REASONING_LEVELS = new Set(["off", "minimal", "low", "medium", "high", "xhigh", "max"]);
const CACHE_RETENTIONS = new Set(["none", "short", "long"]);

export type JsonRecord = Record<string, unknown>;

interface ClientBase {
  version: 1;
  token: string;
}

export interface HelloMessage extends ClientBase {
  type: "hello";
}

export interface ListModelsMessage extends ClientBase {
  type: "list_models";
  id: string;
}

export interface GatewayRequestOptions {
  reasoning?: "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max";
  maxTokens?: number;
  temperature?: number;
  metadata?: JsonRecord;
  sessionId?: string;
  cacheRetention?: "none" | "short" | "long";
}

export interface StreamRequestMessage extends ClientBase {
  type: "request";
  id: string;
  origin: "claudex";
  provider: string;
  modelId: string;
  system: unknown;
  messages: unknown[];
  tools: unknown[];
  options: GatewayRequestOptions;
}

export interface CancelMessage extends ClientBase {
  type: "cancel";
  id: string;
}

export interface WebSearchRequest extends ClientBase {
  type: "web_search";
  id: string;
  provider: string;
  modelId: string;
  query: string;
}

export type ClientMessage =
  | HelloMessage
  | ListModelsMessage
  | StreamRequestMessage
  | CancelMessage
  | WebSearchRequest;
export type ServerMessage = JsonRecord & { version: 1; type: string; id?: string };

export interface WebSearchResult {
  title: string;
  url: string;
  snippet: string;
}

export interface WebSearchResultMessage extends ServerMessage {
  type: "web_search_result";
  id: string;
  provider: string;
  modelId: string;
  results: WebSearchResult[];
}

export interface WebSearchErrorMessage extends ServerMessage {
  type: "web_search_error";
  id: string;
  provider: string;
  modelId: string;
  message: string;
}

export function isRecord(value: unknown): value is JsonRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function serverMessage(type: string, fields: JsonRecord = {}): ServerMessage {
  return { version: PROTOCOL_VERSION, type, ...fields };
}

function parseJsonObject(line: string): JsonRecord {
  try {
    const value: unknown = JSON.parse(line);
    if (isRecord(value)) {
      return value;
    }
    throw new GatewayError("Gateway message must be a JSON object");
  } catch (error) {
    if (error instanceof GatewayError) {
      throw error;
    }
    throw new GatewayError(
      `Invalid gateway JSON: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
}

function requiredString(value: JsonRecord, key: string, requestId?: string): string {
  const result = value[key];
  if (typeof result !== "string" || result.length === 0) {
    throw new GatewayError(`Gateway field ${key} must be a non-empty string`, requestId);
  }
  return result;
}

function requiredIdentifier(value: JsonRecord, key: string, requestId?: string): string {
  const identifier = requiredString(value, key, requestId);
  if (identifier.length > MAX_IDENTIFIER_LENGTH) {
    throw new GatewayError(`Gateway field ${key} is too long`, requestId);
  }
  return identifier;
}

function requiredArray(value: JsonRecord, key: string, requestId: string): unknown[] {
  const result = value[key];
  if (!Array.isArray(result)) {
    throw new GatewayError(`Gateway field ${key} must be an array`, requestId);
  }
  return result;
}

function optionalString(value: JsonRecord, key: string, requestId: string): string | undefined {
  const result = value[key];
  if (result === undefined) {
    return undefined;
  }
  if (typeof result !== "string" || result.length === 0) {
    throw new GatewayError(`Gateway option ${key} must be a non-empty string`, requestId);
  }
  return result;
}

function optionalPositiveInteger(
  value: JsonRecord,
  key: string,
  requestId: string,
): number | undefined {
  const result = value[key];
  if (result === undefined) {
    return undefined;
  }
  if (typeof result !== "number" || !Number.isSafeInteger(result) || result <= 0) {
    throw new GatewayError(`Gateway option ${key} must be a positive integer`, requestId);
  }
  return result;
}

function optionalFiniteNumber(
  value: JsonRecord,
  key: string,
  requestId: string,
): number | undefined {
  const result = value[key];
  if (result === undefined) {
    return undefined;
  }
  if (typeof result !== "number" || !Number.isFinite(result)) {
    throw new GatewayError(`Gateway option ${key} must be finite`, requestId);
  }
  return result;
}

function optionalEnum(
  value: JsonRecord,
  key: string,
  allowed: ReadonlySet<string>,
  requestId: string,
): string | undefined {
  const result = value[key];
  if (result === undefined) {
    return undefined;
  }
  if (typeof result !== "string" || !allowed.has(result)) {
    throw new GatewayError(`Gateway option ${key} is invalid`, requestId);
  }
  return result;
}

function optionalReasoning(value: JsonRecord, id: string): GatewayRequestOptions["reasoning"] {
  const reasoning = optionalEnum(value, "reasoning", REASONING_LEVELS, id);
  if (
    reasoning === "off" ||
    reasoning === "minimal" ||
    reasoning === "low" ||
    reasoning === "medium"
  ) {
    return reasoning;
  }
  if (reasoning === "high" || reasoning === "xhigh" || reasoning === "max") {
    return reasoning;
  }
  return undefined;
}

function optionalCacheRetention(
  value: JsonRecord,
  id: string,
): GatewayRequestOptions["cacheRetention"] {
  const retention = optionalEnum(value, "cacheRetention", CACHE_RETENTIONS, id);
  if (retention === "none" || retention === "short" || retention === "long") {
    return retention;
  }
  return undefined;
}

function parseSamplingOptions(value: JsonRecord, id: string): GatewayRequestOptions {
  const reasoning = optionalReasoning(value, id);
  const maxTokens = optionalPositiveInteger(value, "maxTokens", id);
  const temperature = optionalFiniteNumber(value, "temperature", id);
  const options: GatewayRequestOptions = {};
  if (reasoning !== undefined) {
    options.reasoning = reasoning;
  }
  if (maxTokens !== undefined) {
    options.maxTokens = maxTokens;
  }
  if (temperature !== undefined) {
    options.temperature = temperature;
  }
  return options;
}

function parseSessionOptions(value: JsonRecord, id: string): GatewayRequestOptions {
  const cacheRetention = optionalCacheRetention(value, id);
  const sessionId = optionalString(value, "sessionId", id);
  const { metadata } = value;
  if (metadata !== undefined && !isRecord(metadata)) {
    throw new GatewayError("Gateway option metadata must be an object", id);
  }
  const options: GatewayRequestOptions = {};
  if (metadata !== undefined) {
    options.metadata = metadata;
  }
  if (sessionId !== undefined) {
    options.sessionId = sessionId;
  }
  if (cacheRetention !== undefined) {
    options.cacheRetention = cacheRetention;
  }
  return options;
}

function parseOptions(value: unknown, id: string): GatewayRequestOptions {
  if (!isRecord(value)) {
    throw new GatewayError("Gateway request options must be an object", id);
  }
  return { ...parseSamplingOptions(value, id), ...parseSessionOptions(value, id) };
}

function assertVersionAndToken(value: JsonRecord): void {
  if (value["version"] !== PROTOCOL_VERSION) {
    throw new GatewayError(`Gateway protocol version must be ${PROTOCOL_VERSION}`);
  }
  requiredString(value, "token");
}

function parseStreamRequest(value: JsonRecord, id: string): StreamRequestMessage {
  if (value["origin"] !== CLAUDEX_ORIGIN) {
    throw new GatewayError("Gateway request origin must be claudex", id);
  }
  const provider = requiredIdentifier(value, "provider", id);
  if (provider === "claudex") {
    throw new GatewayError("Gateway recursion rejected provider claudex", id);
  }
  return {
    version: PROTOCOL_VERSION,
    type: "request",
    id,
    token: requiredString(value, "token", id),
    origin: CLAUDEX_ORIGIN,
    provider,
    modelId: requiredIdentifier(value, "modelId", id),
    system: value["system"],
    messages: requiredArray(value, "messages", id),
    tools: requiredArray(value, "tools", id),
    options: parseOptions(value["options"], id),
  };
}

export function parseClientMessage(line: string): ClientMessage {
  const value = parseJsonObject(line);
  assertVersionAndToken(value);
  const type = requiredString(value, "type");
  if (type === "hello") {
    return { version: PROTOCOL_VERSION, type, token: requiredString(value, "token") };
  }
  const id = requiredIdentifier(value, "id");
  if (type === "list_models" || type === "cancel") {
    return { version: PROTOCOL_VERSION, type, id, token: requiredString(value, "token") };
  }
  if (type === "web_search") {
    return {
      version: PROTOCOL_VERSION,
      type,
      id,
      token: requiredString(value, "token"),
      provider: requiredIdentifier(value, "provider", id),
      modelId: requiredIdentifier(value, "modelId", id),
      query: requiredString(value, "query"),
    };
  }
  if (type !== "request") {
    throw new GatewayError(`Unsupported gateway message type: ${type}`, id);
  }
  return parseStreamRequest(value, id);
}
