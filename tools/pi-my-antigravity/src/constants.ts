// This file runs with bun.
/**
 * Wire constants shared by every module.
 *
 * The endpoint and headers match the installed official CLI (`agy` 1.5.9).
 * OAuth client credentials are read from environment variables and are never
 * hardcoded in source files.
 */

import { execFileSync } from "node:child_process";

interface VersionState {
  detected: string | undefined;
}

export interface OAuthClientConfig {
  clientId: string;
  clientSecret: string;
}

export const PROVIDER_ID = "my-antigravity";
export const PROVIDER_NAME = "Antigravity (my)";
export const API_ID = "my-antigravity-api";

/** `agy` talks to the daily Cloud Code Assist host first, then production. */
export const ENDPOINTS: readonly string[] = [
  "https://daily-cloudcode-pa.googleapis.com",
  "https://cloudcode-pa.googleapis.com",
] satisfies readonly string[];

/** Fallback when the installed CLI cannot be queried. */
export const CLI_VERSION = "1.5.9";

const CLI_VERSION_PATTERN = /(\d+\.\d+\.\d+)/;
const CLI_QUERY_TIMEOUT_MS = 2000;
const VERSION_STATE: VersionState = { detected: undefined };

/** Go's `runtime.GOOS` naming, which is what the CLI reports. */
const GO_OS: ReadonlyMap<string, string> = new Map([
  ["darwin", "darwin"],
  ["linux", "linux"],
  ["win32", "windows"],
]);

/** Go's `runtime.GOARCH` naming, which is what the CLI reports. */
const GO_ARCH: ReadonlyMap<string, string> = new Map([
  ["arm64", "arm64"],
  ["x64", "amd64"],
  ["ia32", "386"],
  ["arm", "arm"],
]);

/** Same scopes `agy` requests for Cloud Code Assist. */
export const SCOPES: readonly string[] = [
  "https://www.googleapis.com/auth/aicode",
  "https://www.googleapis.com/auth/cloud-platform",
  "https://www.googleapis.com/auth/userinfo.email",
  "https://www.googleapis.com/auth/userinfo.profile",
  "https://www.googleapis.com/auth/cclog",
  "https://www.googleapis.com/auth/experimentsandconfigs",
] satisfies readonly string[];

export const AUTH_URL = "https://accounts.google.com/o/oauth2/v2/auth";
export const TOKEN_URL = "https://oauth2.googleapis.com/token";
export const CALLBACK_HOST = "127.0.0.1";
export const CALLBACK_PORT = 51123;
export const CALLBACK_PATH = "/oauth-callback";

const firstEnv = (primary: string, fallback: string): string =>
  process.env[primary]?.trim() || process.env[fallback]?.trim() || "";

/** Read OAuth client credentials from environment variables without hardcoding them. */
export function oauthClientConfig(): OAuthClientConfig {
  const clientId = firstEnv("MY_ANTIGRAVITY_CLIENT_ID", "ANTIGRAVITY_CLIENT_ID");
  const clientSecret = firstEnv("MY_ANTIGRAVITY_CLIENT_SECRET", "ANTIGRAVITY_CLIENT_SECRET");
  if (!clientId || !clientSecret) {
    throw new Error(
      "Missing OAuth client config: set MY_ANTIGRAVITY_CLIENT_ID and MY_ANTIGRAVITY_CLIENT_SECRET.",
    );
  }
  return { clientId, clientSecret };
}

/**
 * The CLI builds its User-Agent from `<product>/<version> (aidev_client; os_type=%s;
 * …; auth_method=%s)`. Read the version from the installed binary so a CLI update
 * does not leave a stale fingerprint behind.
 */
export function cliVersion(): string {
  if (VERSION_STATE.detected !== undefined) {
    return VERSION_STATE.detected;
  }
  try {
    const output = execFileSync("agy", ["--version"], {
      encoding: "utf8",
      timeout: CLI_QUERY_TIMEOUT_MS,
    });
    const match = CLI_VERSION_PATTERN.exec(output);
    if (match?.[1]) {
      VERSION_STATE.detected = match[1];
    }
  } catch (error) {
    // Installed `agy` binary is optional; fall back to `CLI_VERSION` when absent.
    if ((error as { code?: string }).code !== "ENOENT") {
      console.warn(
        `my-antigravity: failed to read agy --version: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
  }
  return VERSION_STATE.detected ?? CLI_VERSION;
}

/** CLI-equivalent client fingerprint. */
export function userAgent(): string {
  const override = process.env["MY_ANTIGRAVITY_USER_AGENT"]?.trim();
  if (override) {
    return override;
  }
  const osName = GO_OS.get(process.platform) ?? process.platform;
  const archName = GO_ARCH.get(process.arch) ?? process.arch;
  return `antigravity/cli/${cliVersion()} (aidev_client; os_type=${osName}; arch=${archName}; purpose=agent; auth_method=consumer)`;
}
