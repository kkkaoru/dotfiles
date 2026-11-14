// This TypeScript file is executed with Bun.
import { newestFirst } from "../../claude-plamo-translate/hooks/queue.ts";
import { plamoPrompt } from "../../claude-plamo-translate/hooks/translate.ts";
import { isRecord } from "./settings.ts";

const HOST = "http://127.0.0.1";
const MLX_SERVER = ".local/bin/mlx_lm.server";
const MLX_MODEL = ".local/share/plamo-translate/plamo-3-translate-0.8b-mlx-4bit";
const LLAMA_SERVER = ".local/share/plamo-translate/llama.cpp/llama-server";
const GGUF_MODEL = ".local/share/plamo-translate/plamo-3-translate-0.8b-Q4_K_M.gguf";
const LLAMA_CONTEXT = "16384";
const WINDOWS = "win32";
const STOP = ["<|plamo:tag|>", "<|plamo:bos|>"];
const TEMPERATURE = 0;
const TOKENS_PER_CHAR = 4;
const MIN_TOKENS = 64;
const MAX_TOKENS = 4096;
const READY_TIMEOUT_MS = 60_000;
const READY_POLL_MS = 500;
const PARALLEL_REQUESTS = 4;
export const SETUP_HINT =
  "plamo-translate: the PLaMo server is not reachable; run scripts/setup-plamo-translate.sh. Text is passed through untranslated.";

export interface SpawnSpec {
  readonly args: readonly string[];
  readonly command: string;
}

export interface Child {
  once(event: "error", listener: (error: Error) => void): void;
  unref(): void;
}

export interface ResponseLike {
  readonly ok: boolean;
  readonly status: number;
  json(): Promise<unknown>;
}

export interface RequestInitLike {
  readonly body: string;
  readonly headers: Record<string, string>;
  readonly method: string;
}

export type FetchLike = (url: string, init?: RequestInitLike) => Promise<ResponseLike>;

export interface ServerDeps {
  readonly exists: (path: string) => boolean;
  readonly fetch: FetchLike;
  readonly home: string;
  readonly now: () => number;
  readonly platform: string;
  readonly report: (message: string) => void;
  readonly sleep: (ms: number) => Promise<void>;
  readonly spawn: (command: string, args: readonly string[]) => Child;
}

export interface Server {
  complete(text: string, lang: string): Promise<string | null>;
  ensure(): Promise<boolean>;
}

interface ServerState {
  failed: boolean;
  starting: Promise<boolean> | null;
}

const maxTokens = (text: string): number =>
  Math.min(MAX_TOKENS, MIN_TOKENS + text.length * TOKENS_PER_CHAR);

const joined = (home: string, relative: string): string => `${home}/${relative}`;

function mlxSpec(home: string, port: number, command: string): SpawnSpec {
  return {
    command,
    args: ["--model", joined(home, MLX_MODEL), "--port", String(port), "--trust-remote-code"],
  };
}

function llamaSpec(home: string, port: number, command: string): SpawnSpec {
  return {
    command,
    args: ["-m", joined(home, GGUF_MODEL), "-c", LLAMA_CONTEXT, "--port", String(port)],
  };
}

export function commandSpec(
  home: string,
  platform: string,
  port: number,
  exists: (path: string) => boolean,
): SpawnSpec | undefined {
  const mlx = joined(home, MLX_SERVER);
  if (exists(mlx)) {
    return mlxSpec(home, port, mlx);
  }
  const command = `${joined(home, LLAMA_SERVER)}${platform === WINDOWS ? ".exe" : ""}`;
  if (!exists(command)) {
    return undefined;
  }
  return llamaSpec(home, port, command);
}

function codeOf(error: unknown): unknown {
  return isRecord(error) ? error["code"] : undefined;
}

export function isRefused(error: unknown): boolean {
  if (codeOf(error) === "ECONNREFUSED") {
    return true;
  }
  if (!isRecord(error)) {
    return false;
  }
  const { cause, errors } = error;
  if (isRefused(cause)) {
    return true;
  }
  return Array.isArray(errors) && errors.some((item) => isRefused(item));
}

function isUnknownArray(value: unknown): value is readonly unknown[] {
  return Array.isArray(value);
}

function choiceText(choice: unknown): string | null {
  if (!isRecord(choice) || typeof choice["text"] !== "string") {
    return null;
  }
  return choice["text"];
}

export function completionText(body: unknown): string | null {
  if (!isRecord(body)) {
    return null;
  }
  const { choices } = body;
  if (!isUnknownArray(choices)) {
    return null;
  }
  return choiceText(choices[0]);
}

async function healthy(deps: ServerDeps, port: number): Promise<boolean> {
  try {
    const models = await deps.fetch(`${HOST}:${String(port)}/v1/models`);
    return models.ok;
  } catch (error: unknown) {
    if (isRefused(error)) {
      return false;
    }
    throw error;
  }
}

async function waitReady(deps: ServerDeps, port: number, deadline: number): Promise<boolean> {
  if (await healthy(deps, port)) {
    return true;
  }
  if (deps.now() >= deadline) {
    return false;
  }
  await deps.sleep(READY_POLL_MS);
  return waitReady(deps, port, deadline);
}

function launch(deps: ServerDeps, spec: SpawnSpec): void {
  const child = deps.spawn(spec.command, spec.args);
  child.once("error", (error: Error) => {
    deps.report(`plamo-translate: starting the PLaMo server failed: ${error.message}`);
  });
  child.unref();
}

async function start(deps: ServerDeps, port: number, state: ServerState): Promise<boolean> {
  const spec = commandSpec(deps.home, deps.platform, port, deps.exists);
  if (spec === undefined) {
    state.failed = true;
    deps.report(SETUP_HINT);
    return false;
  }
  launch(deps, spec);
  const ready = await waitReady(deps, port, deps.now() + READY_TIMEOUT_MS);
  state.failed = !ready;
  state.starting = null;
  if (!ready) {
    deps.report(SETUP_HINT);
  }
  return ready;
}

async function ensure(deps: ServerDeps, port: number, state: ServerState): Promise<boolean> {
  if (await healthy(deps, port)) {
    return true;
  }
  if (state.failed) {
    return false;
  }
  state.starting ??= start(deps, port, state);
  return state.starting;
}

async function complete(
  deps: ServerDeps,
  port: number,
  text: string,
  lang: string,
): Promise<string | null> {
  const response = await deps.fetch(`${HOST}:${String(port)}/v1/completions`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      prompt: plamoPrompt(text, lang),
      max_tokens: maxTokens(text),
      temperature: TEMPERATURE,
      stop: STOP,
    }),
  });
  if (!response.ok) {
    throw new Error(`plamo-translate: completion failed: ${String(response.status)}`);
  }
  const body = await response.json();
  return completionText(body);
}

export function createServer(deps: ServerDeps, port: number): Server {
  const state: ServerState = { failed: false, starting: null };
  const enqueue = newestFirst(PARALLEL_REQUESTS);
  return {
    ensure: () => ensure(deps, port, state),
    complete: (text, lang) => enqueue(() => complete(deps, port, text, lang)),
  };
}
