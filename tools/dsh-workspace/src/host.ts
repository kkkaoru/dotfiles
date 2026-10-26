/** Runs with bun. Drives the Host's `workspace/create` RPC through a short-lived `dsh web`. */

import type {
  CreateWorkspaceResult,
  Effects,
  FetchResponse,
  SpawnedHost,
  WorkspaceRecord,
} from "./types.ts";

const POLL_INTERVAL_MS = 500;
const POLL_ATTEMPTS = 120;
const COOKIE_TERMINATOR = ";";
const CREATE_ENDPOINT = "/api/workspace/create";
const CREATE_METHOD = "workspace/create";
const RPC_ID = "dsh-workspace";
const WORKSPACE_ID_FIELD = "workspaceId";
const PATH_FIELD = "path";
const TIMEOUT_MESSAGE = "dsh web did not print its URL within 60s";
const AUTH_MESSAGE = "could not authenticate to the Host";
const REQUEST_MESSAGE = "workspace/create request failed";

interface CreateInput {
  readonly effects: Effects;
  readonly directory: string;
  readonly title: string;
}

interface WaitInput {
  readonly effects: Effects;
  readonly host: SpawnedHost;
  readonly logPath: string;
  readonly port: number;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parseJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}

function hostUrlFor(port: number): RegExp {
  return new RegExp(`http://127\\.0\\.0\\.1:${String(port)}/\\?token=[A-Za-z0-9_-]+`, "u");
}

/** The printed URL, once the Host has written it. */
export function findHostUrl(logText: string, port: number): string | undefined {
  return hostUrlFor(port).exec(logText)?.at(0);
}

/** `scheme://authority` of a printed Host URL. */
export function hostOrigin(url: string): string {
  const match = /^http:\/\/[^/]+/u.exec(url);
  return match === null ? url : (match.at(0) ?? url);
}

/** First cookie pair of a `set-cookie` header. */
export function sessionCookie(setCookie: string | undefined): string | undefined {
  return setCookie?.split(COOKIE_TERMINATOR).at(0);
}

/** Request envelope the Host's typed gateway expects. */
export function createRequestBody(directory: string, title: string): string {
  return JSON.stringify({
    type: "client-request",
    rpcId: RPC_ID,
    method: CREATE_METHOD,
    payload: { args: { request: { path: directory, title } } },
  });
}

function errorMessage(error: unknown, fallback: string): string {
  if (!isRecord(error)) {
    return fallback;
  }
  const { code: rawCode, message: rawMessage } = error;
  const code = typeof rawCode === "string" ? rawCode : fallback;
  const message = typeof rawMessage === "string" ? rawMessage : fallback;
  return `${code}: ${message}`;
}

function parseWorkspace(value: unknown): WorkspaceRecord | undefined {
  if (!isRecord(value)) {
    return undefined;
  }
  const workspaceId = value[WORKSPACE_ID_FIELD];
  const path = value[PATH_FIELD];
  return typeof workspaceId === "string" && typeof path === "string"
    ? { workspaceId, path }
    : undefined;
}

/**
 * Interpret the Host's reply.
 * @param response - the HTTP exchange.
 * @returns a created or existing record, or the Host's error text.
 */
export function parseCreateResponse(response: FetchResponse): CreateWorkspaceResult {
  if (response.status >= 400) {
    return { kind: "error", message: `${REQUEST_MESSAGE}: HTTP ${String(response.status)}` };
  }
  const envelope = parseJson(response.body);
  if (!isRecord(envelope)) {
    return { kind: "error", message: REQUEST_MESSAGE };
  }
  const { result } = envelope;
  if (!isRecord(result)) {
    return { kind: "error", message: REQUEST_MESSAGE };
  }
  const { ok, value, error } = result;
  if (ok !== true) {
    return { kind: "error", message: errorMessage(error, REQUEST_MESSAGE) };
  }
  if (!isRecord(value)) {
    return { kind: "error", message: REQUEST_MESSAGE };
  }
  const workspace = parseWorkspace(value["workspace"]);
  if (workspace === undefined) {
    return { kind: "error", message: REQUEST_MESSAGE };
  }
  return value["created"] === true ? { kind: "created", workspace } : { kind: "exists", workspace };
}

async function waitForUrl(input: WaitInput, attempt: number): Promise<string | undefined> {
  if (attempt >= POLL_ATTEMPTS || !input.host.running()) {
    return undefined;
  }
  const url = findHostUrl(input.effects.readText(input.logPath) ?? "", input.port);
  if (url !== undefined) {
    return url;
  }
  await input.effects.sleep(POLL_INTERVAL_MS);
  return waitForUrl(input, attempt + 1);
}

async function postCreate(
  input: CreateInput,
  endpoint: string,
  cookie: string | undefined,
): Promise<FetchResponse> {
  return input.effects.fetchResponse({
    url: endpoint,
    method: "POST",
    body: createRequestBody(input.directory, input.title),
    ...(cookie === undefined ? {} : { cookie }),
  });
}

/**
 * Register the workspace through a temporary Host.
 * @param input - effects plus the directory and title to register.
 * @returns the Host's outcome; the Host is always stopped.
 */
export async function createWorkspace(input: CreateInput): Promise<CreateWorkspaceResult> {
  const port = await input.effects.freePort();
  const logPath = input.effects.temporaryFile("dsh-workspace");
  const host = input.effects.startHost({ port, logPath });
  try {
    const url = await waitForUrl({ effects: input.effects, host, logPath, port }, 0);
    if (url === undefined) {
      return { kind: "error", message: TIMEOUT_MESSAGE };
    }
    const auth = await input.effects.fetchResponse({ url, method: "GET" });
    if (auth.status >= 400) {
      return { kind: "error", message: AUTH_MESSAGE };
    }
    const response = await postCreate(
      input,
      `${hostOrigin(url)}${CREATE_ENDPOINT}`,
      sessionCookie(auth.setCookie),
    );
    return parseCreateResponse(response);
  } finally {
    host.stop();
    input.effects.removeFile(logPath);
  }
}
