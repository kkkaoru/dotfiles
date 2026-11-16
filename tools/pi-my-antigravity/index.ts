// This file runs with bun.
import type { OAuthCredentials } from "@earendil-works/pi-ai";
import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";

import {
  CONSUMER_PROJECT,
  fetchAvailableModels,
  fetchQuotaSummary,
  fetchUserInfo,
  loadCodeAssistProject,
  rememberModelEnums,
  type DiscoveredModel,
  type QuotaBucket,
  type QuotaGroup,
  type QuotaSummary,
} from "./src/client.ts";
import {
  API_ID,
  DEFAULT_ENDPOINT,
  ENDPOINTS,
  PROVIDER_ID,
  PROVIDER_NAME,
} from "./src/constants.ts";
import { stringifyJson } from "./src/json.ts";
import { mergeDiscovered, staticModels } from "./src/models.ts";
import { login, refreshCredential, type AntigravityCredential } from "./src/oauth.ts";
import { parseAuthValue, streamAntigravity } from "./src/stream.ts";
import { closePool } from "./src/transport.ts";

interface ProjectState {
  resolved: string | undefined;
}

interface LoginCallbacks {
  onAuth(info: { url: string; instructions?: string }): void;
  signal?: AbortSignal;
}

/** `loadCodeAssist` project, resolved once per process and shared by request and catalog paths. */
const PROJECT_STATE: ProjectState = { resolved: undefined };

async function projectFor(token: string): Promise<string> {
  if (PROJECT_STATE.resolved) {
    return PROJECT_STATE.resolved;
  }
  PROJECT_STATE.resolved = (await loadCodeAssistProject(token)) ?? CONSUMER_PROJECT;
  return PROJECT_STATE.resolved;
}

function asCredential(
  credentials: OAuthCredentials | undefined,
): AntigravityCredential | undefined {
  if (!credentials || typeof credentials.access !== "string") {
    return undefined;
  }
  return credentials as AntigravityCredential;
}

async function loginAntigravity(callbacks: LoginCallbacks): Promise<OAuthCredentials> {
  const credential = await login((url) => {
    callbacks.onAuth({
      url,
      instructions:
        "Complete the Google sign-in; the browser returns to this machine automatically.",
    });
  });
  // Same startup reads the CLI performs after sign-in.
  const [projectResult, userResult] = await Promise.allSettled([
    loadCodeAssistProject(credential.access),
    fetchUserInfo(credential.access),
  ]);
  if (projectResult.status === "fulfilled" && projectResult.value) {
    PROJECT_STATE.resolved = projectResult.value;
    credential.projectId = projectResult.value;
  } else if (projectResult.status === "rejected") {
    console.warn(
      `my-antigravity: loadCodeAssist failed during login: ${projectResult.reason instanceof Error ? projectResult.reason.message : String(projectResult.reason)}`,
    );
  }
  if (userResult.status === "fulfilled" && userResult.value.email && !credential.email) {
    credential.email = userResult.value.email;
  } else if (userResult.status === "rejected") {
    console.warn(
      `my-antigravity: fetchUserInfo failed during login: ${userResult.reason instanceof Error ? userResult.reason.message : String(userResult.reason)}`,
    );
  }
  return credential as OAuthCredentials;
}

const PERCENT_SCALE = 100;
const JST_FORMATTER = new Intl.DateTimeFormat("sv-SE", {
  timeZone: "Asia/Tokyo",
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
  hour12: false,
});

function formatResetTimeJst(resetTime: string): string {
  const date = new Date(resetTime);
  if (Number.isNaN(date.getTime())) {
    return resetTime;
  }
  return `${JST_FORMATTER.format(date)} JST`;
}

function formatRemainingAndReset(
  remainingFraction: number | undefined,
  resetTime: string | undefined,
): string {
  const remaining =
    remainingFraction !== undefined
      ? `${(remainingFraction * PERCENT_SCALE).toFixed(1)}% remaining`
      : "remaining unknown";
  const reset = resetTime ? `, resets ${formatResetTimeJst(resetTime)}` : "";
  return `${remaining}${reset}`;
}

function formatQuotaBucket(bucket: QuotaBucket): string {
  const label = bucket.window ?? bucket.displayName ?? bucket.bucketId;
  return `  ${label}: ${formatRemainingAndReset(bucket.remainingFraction, bucket.resetTime)}`;
}

function formatQuotaGroup(group: QuotaGroup): string {
  const lines = [group.displayName, ...group.buckets.map(formatQuotaBucket)];
  return lines.join("\n");
}

function formatQuotaOutput(summary: QuotaSummary, discovered: readonly DiscoveredModel[]): string {
  if (summary.groups.length > 0) {
    return summary.groups.map(formatQuotaGroup).join("\n");
  }
  const withQuota = discovered.filter((model) => model.quota);
  if (withQuota.length === 0) {
    return "No quota info returned.";
  }
  return withQuota
    .map(
      (model) =>
        `${model.id}: ${formatRemainingAndReset(model.quota?.remainingFraction, model.quota?.resetTime)}`,
    )
    .join("\n");
}

function print(
  ctx: ExtensionCommandContext,
  text: string,
  level: "info" | "warning" = "info",
): void {
  if (ctx.hasUI) {
    ctx.ui.notify(text, level);
  }
  console.log(text);
}

function apiKeyFromCredentials(credentials: OAuthCredentials): string {
  const credential = asCredential(credentials);
  if (!credential) {
    return "";
  }
  const projectId = credential.projectId ?? PROJECT_STATE.resolved;
  return stringifyJson(
    projectId ? { token: credential.access, projectId } : { token: credential.access },
  );
}

export default function piMyAntigravity(pi: ExtensionAPI): void {
  // Defer agy binary parsing until login/refresh needs oauthClientConfig().
  pi.registerProvider(PROVIDER_ID, {
    name: PROVIDER_NAME,
    baseUrl: DEFAULT_ENDPOINT,
    api: API_ID,
    models: staticModels(),
    oauth: {
      name: PROVIDER_NAME,
      isSubscription: true,
      login: loginAntigravity,
      refreshToken: async (credentials: OAuthCredentials) =>
        refreshCredential(asCredential(credentials) as AntigravityCredential),
      // Synchronous by contract: the project is resolved during login/refresh and cached.
      getApiKey: apiKeyFromCredentials,
    },
    /** Live catalog: `fetchAvailableModels` is what `agy models` prints. */
    refreshModels: async (context) => {
      const credential = asCredential(context.credential as OAuthCredentials | undefined);
      if (!credential?.access || !context.allowNetwork) {
        return staticModels();
      }
      const projectId = credential.projectId ?? (await projectFor(credential.access));
      // Account lookup mirrors the CLI's startup sequence; do not block model discovery if user info fails.
      const [, modelsResult] = await Promise.allSettled([
        fetchUserInfo(credential.access, { signal: context.signal }),
        fetchAvailableModels(credential.access, projectId, { signal: context.signal }),
      ]);
      if (modelsResult.status === "rejected") {
        throw modelsResult.reason;
      }
      rememberModelEnums(modelsResult.value);
      return mergeDiscovered(modelsResult.value);
    },
    streamSimple: streamAntigravity,
  });

  // Idle HTTP/2 connections are per process; close them with the session.
  pi.on("session_shutdown", () => closePool());

  pi.registerCommand("my-antigravity.models", {
    description: "List Antigravity runtime models available to this subscription",
    handler: async (_args, ctx) => {
      const raw = await ctx.modelRegistry.getApiKeyForProvider(PROVIDER_ID);
      if (!raw) {
        print(ctx, "No Antigravity credential. Run /login my-antigravity first.", "warning");
        return;
      }
      const { token, projectId } = parseAuthValue(raw);
      const discovered = await fetchAvailableModels(token, projectId ?? CONSUMER_PROJECT);
      print(ctx, ["Antigravity runtime models", ...discovered.map((model) => model.id)].join("\n"));
    },
  });

  pi.registerCommand("my-antigravity.quota", {
    description: "Show Antigravity 5h and weekly quota pools",
    handler: async (_args, ctx) => {
      const raw = await ctx.modelRegistry.getApiKeyForProvider(PROVIDER_ID);
      if (!raw) {
        print(ctx, "No Antigravity credential. Run /login my-antigravity first.", "warning");
        return;
      }
      const { token, projectId } = parseAuthValue(raw);
      const project = projectId ?? CONSUMER_PROJECT;
      const [summaryResult, modelsResult] = await Promise.allSettled([
        fetchQuotaSummary(token, project),
        fetchAvailableModels(token, project),
      ]);
      const summary = summaryResult.status === "fulfilled" ? summaryResult.value : { groups: [] };
      const discovered = modelsResult.status === "fulfilled" ? modelsResult.value : [];
      if (summaryResult.status === "rejected" && modelsResult.status === "rejected") {
        throw summaryResult.reason;
      }
      const output = formatQuotaOutput(summary, discovered);
      const content = `Antigravity quota\n\`\`\`\n${output}\n\`\`\``;
      pi.events.emit("plamo-translate:exclude", content);
      if (ctx.hasUI) {
        pi.sendMessage(
          {
            customType: "my-antigravity-quota",
            content,
            display: true,
          },
          { triggerTurn: false },
        );
      } else {
        console.log(`Antigravity quota\n${output}`);
      }
    },
  });

  pi.registerCommand("my-antigravity.doctor", {
    description: "Show sanitized my-antigravity provider state",
    handler: async (_args, ctx) => {
      print(
        ctx,
        [
          `provider=${PROVIDER_ID}`,
          `api=${API_ID}`,
          `endpoint=${ENDPOINTS[0]}`,
          `project=${PROJECT_STATE.resolved ?? "unresolved"}`,
          "wire=leading user turn carries the system prompt (agy uses systemInstruction; the backend 429s third-party prompts there)",
          "commands=/login my-antigravity, /my-antigravity.models, /my-antigravity.quota, /my-antigravity.doctor",
        ].join("\n"),
      );
    },
  });
}
