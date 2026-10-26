// This file runs with Bun.
import { mkdir, readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { getBuiltinModels } from "@earendil-works/pi-ai/providers/all";
import type { ModelCandidate } from "./claudex-models.ts";

/** The subset of the Agent SDK's `ModelInfo` used here. */
export interface SupportedModel {
  readonly value: string;
  readonly resolvedModel?: string | undefined;
}

export type ListSupportedModels = (
  signal: AbortSignal | undefined,
) => Promise<readonly SupportedModel[]>;

export interface SubscriptionRefresh {
  readonly list: ListSupportedModels;
  readonly cachePath: string;
  readonly signal: AbortSignal | undefined;
}

export const SUBSCRIPTION_MODELS_CACHE = path.join(
  os.homedir(),
  ".cache",
  "claudex",
  "pi-subscription-models.json",
);
const CLAUDE_ID_PREFIX = "claude-";
const UNKNOWN_CONTEXT_WINDOW = 200_000;
// Variables that would redirect the Claude Code child away from the user's own plan login.
const REDIRECTING_ENV: ReadonlySet<string> = new Set([
  "ANTHROPIC_API_KEY",
  "ANTHROPIC_AUTH_TOKEN",
  "ANTHROPIC_BASE_URL",
  "CLAUDE_CONFIG_DIR",
  "CLAUDECODE",
]);
const PARENT_SESSION_PREFIX = "CLAUDE_CODE_";

/* Used until the first live refresh succeeds: the canonical models the Agent SDK listed for the
   plan login on 2026-10-21. */
const FALLBACK_MODEL_IDS: readonly string[] = [
  "claude-opus-5-5",
  "claude-fable-5-1",
  "claude-sonnet-5-5",
  "claude-haiku-5-5",
  "claude-opus-5",
  "claude-fable-5",
  "claude-sonnet-5",
  "claude-opus-4-8",
  "claude-opus-4-7",
  "claude-opus-4-6",
  "claude-sonnet-4-6",
  "claude-haiku-4-5-20251001",
];

function contextWindowFor(id: string): number {
  return (
    getBuiltinModels("anthropic").find((model) => model.id === id)?.contextWindow ??
    UNKNOWN_CONTEXT_WINDOW
  );
}

function toCandidate(id: string): ModelCandidate {
  return { id, contextWindow: contextWindowFor(id) };
}

/** Aliases such as `default` or `opus` resolve to canonical ids; keep each canonical id once. */
export function subscriptionCandidates(models: readonly SupportedModel[]): ModelCandidate[] {
  const ids = models
    .map((model) => model.resolvedModel ?? model.value)
    .filter((id) => id.startsWith(CLAUDE_ID_PREFIX));
  return [...new Set(ids)].map((id) => toCandidate(id));
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item) => typeof item === "string");
}

export async function cachedSubscriptionModels(cachePath: string): Promise<ModelCandidate[]> {
  const ids = await readFile(cachePath, "utf8")
    .then((text): unknown => JSON.parse(text))
    .catch(() => null);
  return (isStringArray(ids) && ids.length > 0 ? ids : FALLBACK_MODEL_IDS).map((id) =>
    toCandidate(id),
  );
}

/** Lists the plan's models live and caches the ids; a failed lookup keeps the cached list. */
export async function refreshSubscriptionModels(
  refresh: SubscriptionRefresh,
): Promise<ModelCandidate[]> {
  const live = await refresh
    .list(refresh.signal)
    .then((models) => subscriptionCandidates(models))
    .catch(() => []);
  if (live.length === 0) {
    return cachedSubscriptionModels(refresh.cachePath);
  }
  await mkdir(path.dirname(refresh.cachePath), { recursive: true });
  await writeFile(refresh.cachePath, `${JSON.stringify(live.map((model) => model.id))}\n`);
  return live;
}

function childEnv(env: NodeJS.ProcessEnv): Record<string, string | undefined> {
  return Object.fromEntries(
    Object.entries(env).filter(
      ([key]) => !REDIRECTING_ENV.has(key) && !key.startsWith(PARENT_SESSION_PREFIX),
    ),
  );
}

/* Initializes the bundled Claude Code under the user's login and reads its model list. This makes
   no model request, so it consumes no usage. */
export async function sdkSupportedModels(
  signal: AbortSignal | undefined,
): Promise<readonly SupportedModel[]> {
  const { query } = await import("@anthropic-ai/claude-agent-sdk");
  const closed = Promise.withResolvers<null>();
  // Streaming input that sends nothing and ends on close keeps the session idle.
  const idle: AsyncIterable<never> = {
    [Symbol.asyncIterator]: () => ({
      next: async () => {
        await closed.promise;
        return { done: true, value: undefined };
      },
    }),
  };
  const abortController = new AbortController();
  signal?.addEventListener("abort", () => abortController.abort(), { once: true });
  const session = query({
    prompt: idle,
    options: {
      abortController,
      tools: [],
      settingSources: [],
      persistSession: false,
      env: childEnv(process.env),
    },
  });
  try {
    return await session.supportedModels();
  } finally {
    closed.resolve(null);
    session.close();
  }
}
