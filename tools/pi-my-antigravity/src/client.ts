// This file runs with bun.
import { API_ID, ENDPOINTS, PROVIDER_ID } from "./constants.ts";
import { isRecord, nonEmptyString, parseJsonOrRaw, stringifyJson } from "./json.ts";
import { postWire, type WireRequest, type WireResponse } from "./transport.ts";

export { API_ID, PROVIDER_ID };

export interface DiscoveredModel {
  /** Runtime id exactly as `fetchAvailableModels` reports it, e.g. `gemini-4-hydrogen-medium`. */
  id: string;
  /** `model` field of the catalog entry: the `model_enum` label the CLI sends. */
  modelEnum?: string;
  displayName?: string;
  supportsThinking?: boolean;
  supportsImages?: boolean;
}

export interface UserInfo {
  email?: string;
  tier?: string;
}

export interface ClientTransportOptions {
  signal?: AbortSignal;
  send?: (request: WireRequest) => Promise<WireResponse>;
}

interface PostJsonOptions {
  path: string;
  token: string;
  payload: string;
  signal?: AbortSignal;
  send: (request: WireRequest) => Promise<WireResponse>;
}

interface PostJsonInput {
  path: string;
  token: string;
  body: Record<string, unknown>;
  signal?: AbortSignal;
  send: (request: WireRequest) => Promise<WireResponse>;
}

type AttemptOutcome = { ok: true; value: unknown } | { ok: false; error: string };

/**
 * `agy` resolves the project through `loadCodeAssist`; `aicode-consumers` is what a
 * consumer subscription reports, so it is also the fallback when the call is skipped.
 */
export const CONSUMER_PROJECT = "aicode-consumers";

const HTTP_OK_MIN = 200;
const HTTP_OK_MAX = 300;
const MAX_ERROR_BODY_LENGTH = 200;

/** Runtime id → `model_enum`, filled from the live catalog. */
const MODEL_ENUMS: Map<string, string> = new Map();

function messageOf(body: unknown, fallback: string): string {
  if (!isRecord(body)) {
    return fallback;
  }
  const error = body["error"];
  if (!isRecord(error)) {
    return fallback;
  }
  return nonEmptyString(error["message"]) ?? fallback;
}

async function tryEndpoint(endpoint: string, options: PostJsonOptions): Promise<AttemptOutcome> {
  try {
    const response = await options.send({
      url: `${endpoint}${options.path}`,
      token: options.token,
      body: options.payload,
      accept: "application/json",
      ...(options.signal ? { signal: options.signal } : {}),
    });
    const text = await response.text();
    const parsed = parseJsonOrRaw(text);
    if (response.status >= HTTP_OK_MIN && response.status < HTTP_OK_MAX) {
      return { ok: true, value: parsed };
    }
    return {
      ok: false,
      error: `${response.status}: ${messageOf(parsed, text.slice(0, MAX_ERROR_BODY_LENGTH))}`,
    };
  } catch (error) {
    return {
      ok: false,
      error: error instanceof Error ? error.message : String(error),
    };
  }
}

async function postJsonAt(
  index: number,
  options: PostJsonOptions,
  lastError: string,
): Promise<unknown> {
  const endpoint = ENDPOINTS[index];
  if (!endpoint) {
    throw new Error(`${options.path} failed: ${lastError || "no endpoint reachable"}`);
  }
  const outcome = await tryEndpoint(endpoint, options);
  return outcome.ok ? outcome.value : await postJsonAt(index + 1, options, outcome.error);
}

const postJson = (input: PostJsonInput): Promise<unknown> =>
  postJsonAt(
    0,
    {
      path: input.path,
      token: input.token,
      payload: stringifyJson(input.body),
      ...(input.signal ? { signal: input.signal } : {}),
      send: input.send,
    },
    "",
  );

export function rememberModelEnums(discovered: DiscoveredModel[]): void {
  for (const model of discovered) {
    if (model.modelEnum) {
      MODEL_ENUMS.set(model.id, model.modelEnum);
    }
  }
}

export const modelEnumFor = (runtimeId: string): string | undefined => MODEL_ENUMS.get(runtimeId);

const buildUserInfo = (email: unknown, tier: unknown): UserInfo => ({
  ...(typeof email === "string" && email ? { email } : {}),
  ...(typeof tier === "string" ? { tier } : {}),
});

function toUserInfo(data: unknown): UserInfo {
  if (!isRecord(data)) {
    return {};
  }
  const nestedUser = data["user"];
  const nestedEmail = isRecord(nestedUser) ? nestedUser["email"] : undefined;
  return buildUserInfo(data["email"] ?? nestedEmail, data["userTier"]);
}

/** `v1internal:fetchUserInfo` — the CLI reads the signed-in account on startup. */
export async function fetchUserInfo(
  token: string,
  options?: ClientTransportOptions,
): Promise<UserInfo> {
  const data = await postJson({
    path: "/v1internal:fetchUserInfo",
    token,
    body: {},
    ...(options?.signal ? { signal: options.signal } : {}),
    send: options?.send ?? postWire,
  });
  return toUserInfo(data);
}

export async function loadCodeAssistProject(
  token: string,
  options?: ClientTransportOptions,
): Promise<string | undefined> {
  const data = await postJson({
    path: "/v1internal:loadCodeAssist",
    token,
    body: {
      metadata: { ideType: "ANTIGRAVITY", platform: "PLATFORM_UNSPECIFIED", pluginType: "GEMINI" },
    },
    ...(options?.signal ? { signal: options.signal } : {}),
    send: options?.send ?? postWire,
  });
  if (!isRecord(data)) {
    return undefined;
  }
  const project = data["cloudaicompanionProject"] ?? data["project"];
  return typeof project === "string" && project ? project : undefined;
}

const compareDiscovered = (left: DiscoveredModel, right: DiscoveredModel): number =>
  left.id.localeCompare(right.id);

function parseModelEntry(id: string, info: Record<string, unknown>): DiscoveredModel {
  const model: DiscoveredModel = { id };
  if (typeof info["model"] === "string" && info["model"]) {
    model.modelEnum = info["model"];
  }
  const display = info["displayName"] ?? info["label"] ?? info["modelName"];
  if (typeof display === "string" && display) {
    model.displayName = display;
  }
  if (info["supportsThinking"] === true) {
    model.supportsThinking = true;
  }
  if (info["supportsImages"] === true) {
    model.supportsImages = true;
  }
  return model;
}

const isAllowedCatalogEntry = (id: string, info: unknown): info is Record<string, unknown> =>
  isRecord(info) && info["isInternal"] !== true && !/^(chat_|tab_)/i.test(id) && !/image/i.test(id);

/** `fetchAvailableModels` is what `agy models` prints, including per-tier runtime ids. */
export async function fetchAvailableModels(
  token: string,
  project: string,
  options?: ClientTransportOptions,
): Promise<DiscoveredModel[]> {
  const data = await postJson({
    path: "/v1internal:fetchAvailableModels",
    token,
    body: { project },
    ...(options?.signal ? { signal: options.signal } : {}),
    send: options?.send ?? postWire,
  });
  if (!isRecord(data) || !isRecord(data["models"])) {
    return [];
  }
  const discovered: DiscoveredModel[] = [];
  for (const [id, info] of Object.entries(data["models"])) {
    if (!isAllowedCatalogEntry(id, info)) {
      continue;
    }
    discovered.push(parseModelEntry(id, info));
  }
  return discovered.sort(compareDiscovered);
}
