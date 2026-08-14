import type { OAuthCredentials } from "@earendil-works/pi-ai";
import { readAuthRecords, type AuthKeyOptions } from "./auth.ts";
import { resolveApiBase, WORKOS_TOKEN_PREFIX } from "./env.ts";
import { isRecord, stringValue } from "./utils.ts";

export { WORKOS_TOKEN_PREFIX } from "./env.ts";

export const CLINE_REFRESH_ENDPOINT = "/api/v1/auth/refresh";
export const WORKOS_TOKEN_LIFETIME_MS = 55 * 60 * 1000;
export const WORKOS_REFRESH_MARGIN_MS = 5 * 60 * 1000;
export const WORKOS_REFRESH_TIMEOUT_MS = 15_000;

const CLINE_PROVIDER_KEYS = ["cline-pass", "cline"] as const;

export interface ClineAuthCredentials {
  accessToken: string;
  expiresAt: number;
  refreshToken: string;
}

export interface WorkosRefreshOptions {
  apiBase?: string;
  fetch?: typeof globalThis.fetch;
}

interface RefreshTokens {
  accessToken: string;
  refreshToken: string;
}

export function isWorkosToken(token: string): boolean {
  return token.startsWith(WORKOS_TOKEN_PREFIX);
}

export function credentialsFromWorkos(
  accessToken: string,
  refreshToken: string,
  expiresAt: number,
): OAuthCredentials {
  return { access: accessToken, expires: expiresAt, refresh: refreshToken };
}

function requestSignal(): AbortSignal {
  return AbortSignal.timeout(WORKOS_REFRESH_TIMEOUT_MS);
}

async function refreshResponse(
  credentials: OAuthCredentials,
  options: WorkosRefreshOptions,
): Promise<Response> {
  const fetchFunction = options.fetch ?? globalThis.fetch;
  const apiBase = options.apiBase ?? resolveApiBase();
  try {
    return await fetchFunction(`${apiBase}${CLINE_REFRESH_ENDPOINT}`, {
      body: JSON.stringify({ granttype: "refresh_token", refreshToken: credentials.refresh }),
      headers: { "Content-Type": "application/json" },
      method: "POST",
      signal: requestSignal(),
    });
  } catch (error) {
    if (isRecord(error) && error["name"] === "AbortError") {
      throw new Error(
        "ClinePass token refresh timed out — check your network or try a static API key.",
        { cause: error },
      );
    }
    throw error;
  }
}

function parseRefreshTokens(value: unknown): RefreshTokens | undefined {
  if (!isRecord(value)) {
    return undefined;
  }
  const nested = isRecord(value["data"]) ? value["data"] : value;
  const accessToken = stringValue(nested["accessToken"]);
  const refreshToken = stringValue(nested["refreshToken"]);
  if (accessToken === undefined || refreshToken === undefined) {
    return undefined;
  }
  return { accessToken, refreshToken };
}

export async function refreshWorkosToken(
  credentials: OAuthCredentials,
  options: WorkosRefreshOptions = {},
): Promise<OAuthCredentials> {
  const response = await refreshResponse(credentials, options);
  if (!response.ok) {
    const detail = await response.text().catch(() => "unknown error");
    throw new Error(
      `ClinePass token refresh failed (${response.status}): ${detail} — try running \`cline auth\` to re-login, or use a static API key.`,
    );
  }

  const tokens = parseRefreshTokens(await response.json());
  if (tokens === undefined) {
    throw new Error("ClinePass token refresh returned unexpected response format");
  }
  const accessToken = isWorkosToken(tokens.accessToken)
    ? tokens.accessToken
    : `${WORKOS_TOKEN_PREFIX}${tokens.accessToken}`;
  return credentialsFromWorkos(
    accessToken,
    tokens.refreshToken,
    Date.now() + WORKOS_TOKEN_LIFETIME_MS - WORKOS_REFRESH_MARGIN_MS,
  );
}

function expiry(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

function authRecord(value: unknown): ClineAuthCredentials | undefined {
  if (!isRecord(value)) {
    return undefined;
  }
  const accessToken = stringValue(value["accessToken"]);
  const refreshToken = stringValue(value["refreshToken"]);
  if (accessToken === undefined || refreshToken === undefined || !isWorkosToken(accessToken)) {
    return undefined;
  }
  return { accessToken, expiresAt: expiry(value["expiresAt"]), refreshToken };
}

function piCredential(parsed: Record<string, unknown>): ClineAuthCredentials | undefined {
  const value = parsed["clinepass"];
  if (!isRecord(value)) {
    return undefined;
  }
  const accessToken = stringValue(value["access"]);
  const refreshToken = stringValue(value["refresh"]);
  if (accessToken === undefined || refreshToken === undefined || !isWorkosToken(accessToken)) {
    return undefined;
  }
  return { accessToken, expiresAt: expiry(value["expires"]), refreshToken };
}

function providerCredentials(parsed: Record<string, unknown>): ClineAuthCredentials[] {
  const providers = isRecord(parsed["providers"]) ? parsed["providers"] : {};
  return CLINE_PROVIDER_KEYS.flatMap((providerKey) => {
    const provider = isRecord(providers[providerKey]) ? providers[providerKey] : undefined;
    const settings = isRecord(provider?.["settings"]) ? provider["settings"] : undefined;
    const credential = authRecord(settings?.["auth"]);
    return credential === undefined ? [] : [credential];
  });
}

export function resolveClineAuthCredentials(
  options: AuthKeyOptions = {},
): ClineAuthCredentials | undefined {
  const candidates = readAuthRecords(options).flatMap((parsed) => {
    const piAuth = piCredential(parsed);
    const providerAuth = providerCredentials(parsed);
    if (piAuth !== undefined) {
      providerAuth.unshift(piAuth);
    }
    return providerAuth;
  });
  return candidates.toSorted((left, right) => right.expiresAt - left.expiresAt)[0];
}
