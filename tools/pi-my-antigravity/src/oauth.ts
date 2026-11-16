// This file runs with bun.
import { createHash, randomBytes } from "node:crypto";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { URL, URLSearchParams } from "node:url";

import {
  AUTH_URL,
  CALLBACK_HOST,
  CALLBACK_PATH,
  CALLBACK_PORT,
  SCOPES,
  TOKEN_URL,
  oauthClientConfig,
} from "./constants.ts";
import { isRecord, parseJson } from "./json.ts";
import { httpPost, type HttpPostOptions, type WireResponse } from "./transport.ts";

/** Credential shape pi stores for an OAuth provider. */
export interface AntigravityCredential {
  type: "oauth";
  [key: string]: unknown;
  access: string;
  refresh: string;
  expires: number;
  /** Cloud Code Assist project resolved from `loadCodeAssist`. */
  projectId?: string;
  email?: string;
}

export interface ExchangeCodeOptions {
  code: string;
  redirectUri: string;
  verifier: string;
  post?: (options: HttpPostOptions) => Promise<WireResponse>;
}

export interface RefreshCredentialOptions {
  post?: (options: HttpPostOptions) => Promise<WireResponse>;
}

export interface TokenResponse {
  access_token?: string;
  refresh_token?: string;
  expires_in?: number;
  id_token?: string;
  error?: string;
  error_description?: string;
}

export interface PreviousCredentialMeta {
  refresh?: string;
  projectId?: string;
  email?: string;
}

export interface PkcePair {
  verifier: string;
  challenge: string;
}

interface CallbackState {
  redirectUri: string;
  resolve?: (code: string) => void;
  reject?: (error: Error) => void;
}

const DEFAULT_EXPIRES_IN_SECONDS = 3600;
const MS_PER_SECOND = 1000;
const VERIFIER_BYTES = 32;
const STATE_BYTES = 16;

const base64url = (input: Buffer): string =>
  input.toString("base64").replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");

export function pkcePair(): PkcePair {
  const verifier = base64url(randomBytes(VERIFIER_BYTES));
  const challenge = base64url(createHash("sha256").update(verifier).digest());
  return { verifier, challenge };
}

export function authorizeUrl(redirectUri: string, challenge: string, state: string): string {
  const { clientId } = oauthClientConfig();
  const url = new URL(AUTH_URL);
  url.searchParams.set("client_id", clientId);
  url.searchParams.set("response_type", "code");
  url.searchParams.set("redirect_uri", redirectUri);
  url.searchParams.set("scope", SCOPES.join(" "));
  url.searchParams.set("code_challenge", challenge);
  url.searchParams.set("code_challenge_method", "S256");
  url.searchParams.set("access_type", "offline");
  url.searchParams.set("prompt", "consent");
  url.searchParams.set("state", state);
  return url.toString();
}

/** Decode the `email` claim of an id_token without verifying it (display only). */
export function emailFromIdToken(idToken: string | undefined): string | undefined {
  if (!idToken) {
    return undefined;
  }
  const payload = idToken.split(".")[1];
  if (!payload) {
    return undefined;
  }
  try {
    const parsed = parseJson(Buffer.from(payload, "base64url").toString("utf8"));
    return isRecord(parsed) && typeof parsed["email"] === "string" ? parsed["email"] : undefined;
  } catch (error) {
    console.warn(
      `my-antigravity: failed to decode id_token payload: ${error instanceof Error ? error.message : String(error)}`,
    );
    return undefined;
  }
}

function assertTokenBody(body: TokenResponse): string {
  if (body.error || !body.access_token) {
    throw new Error(
      `Antigravity sign-in failed: ${body.error_description || body.error || "no access token"}`,
    );
  }
  return body.access_token;
}

const applyPreviousMeta = (
  credential: AntigravityCredential,
  email: string | undefined,
  previous?: PreviousCredentialMeta,
): AntigravityCredential => ({
  ...credential,
  ...(previous?.projectId ? { projectId: previous.projectId } : {}),
  ...(email ? { email } : {}),
});

export function credentialFromTokenResponse(
  body: TokenResponse,
  previous?: PreviousCredentialMeta,
): AntigravityCredential {
  const access = assertTokenBody(body);
  const expires = Date.now() + (body.expires_in ?? DEFAULT_EXPIRES_IN_SECONDS) * MS_PER_SECOND;
  const email = emailFromIdToken(body.id_token) ?? previous?.email;
  return applyPreviousMeta(
    {
      type: "oauth",
      access,
      refresh: body.refresh_token || previous?.refresh || "",
      expires,
    },
    email,
    previous,
  );
}

function toTokenResponse(value: unknown): TokenResponse {
  if (!isRecord(value)) {
    return {};
  }
  return {
    ...(typeof value["access_token"] === "string" ? { access_token: value["access_token"] } : {}),
    ...(typeof value["refresh_token"] === "string"
      ? { refresh_token: value["refresh_token"] }
      : {}),
    ...(typeof value["expires_in"] === "number" ? { expires_in: value["expires_in"] } : {}),
    ...(typeof value["id_token"] === "string" ? { id_token: value["id_token"] } : {}),
    ...(typeof value["error"] === "string" ? { error: value["error"] } : {}),
    ...(typeof value["error_description"] === "string"
      ? { error_description: value["error_description"] }
      : {}),
  };
}

export async function exchangeCode({
  code,
  redirectUri,
  verifier,
  post,
}: ExchangeCodeOptions): Promise<AntigravityCredential> {
  const { clientId, clientSecret } = oauthClientConfig();
  const send = post ?? httpPost;
  const response = await send({
    url: TOKEN_URL,
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      client_id: clientId,
      client_secret: clientSecret,
      code,
      grant_type: "authorization_code",
      redirect_uri: redirectUri,
      code_verifier: verifier,
    }).toString(),
  });
  return credentialFromTokenResponse(toTokenResponse(parseJson(await response.text())));
}

export async function refreshCredential(
  credential: AntigravityCredential,
  options?: RefreshCredentialOptions,
): Promise<AntigravityCredential> {
  if (!credential.refresh) {
    throw new Error("Antigravity credential has no refresh token; run /login again");
  }
  const { clientId, clientSecret } = oauthClientConfig();
  const send = options?.post ?? httpPost;
  const response = await send({
    url: TOKEN_URL,
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      client_id: clientId,
      client_secret: clientSecret,
      refresh_token: credential.refresh,
      grant_type: "refresh_token",
    }).toString(),
  });
  const body = toTokenResponse(parseJson(await response.text()));
  const previous: PreviousCredentialMeta = {
    refresh: credential.refresh,
    ...(credential.projectId !== undefined ? { projectId: credential.projectId } : {}),
    ...(credential.email !== undefined ? { email: credential.email } : {}),
  };
  return credentialFromTokenResponse(body, previous);
}

function handleCallbackRequest(
  request: IncomingMessage,
  response: ServerResponse,
  expectedState: string,
  callbackState: CallbackState,
  closeServer: () => void,
): void {
  const incoming = new URL(request.url ?? "/", callbackState.redirectUri);
  if (incoming.pathname !== CALLBACK_PATH) {
    response.writeHead(404).end();
    return;
  }
  const code = incoming.searchParams.get("code");
  const error = incoming.searchParams.get("error");
  const returnedState = incoming.searchParams.get("state");
  response.writeHead(200, { "Content-Type": "text/plain; charset=utf-8" });
  if (error || !code || returnedState !== expectedState) {
    response.end("Antigravity sign-in failed. You can close this tab.");
    closeServer();
    callbackState.reject?.(new Error(error ?? "Antigravity sign-in returned no code"));
    return;
  }
  response.end("Antigravity sign-in complete. You can close this tab.");
  closeServer();
  callbackState.resolve?.(code);
}

/**
 * Run the browser PKCE flow. `onUrl` receives the verification URL; the returned
 * promise resolves once the loopback listener has the code.
 */
export async function login(
  onUrl: (url: string) => void | Promise<void>,
  port = CALLBACK_PORT,
  post?: (options: HttpPostOptions) => Promise<WireResponse>,
): Promise<AntigravityCredential> {
  const state = base64url(randomBytes(STATE_BYTES));
  const { verifier, challenge } = pkcePair();
  const callbackState: CallbackState = {
    redirectUri: `http://${CALLBACK_HOST}:${port}${CALLBACK_PATH}`,
  };

  const codePromise = new Promise<string>((resolve, reject) => {
    callbackState.resolve = resolve;
    callbackState.reject = reject;
  });

  const server = createServer((request, response) =>
    handleCallbackRequest(request, response, state, callbackState, () => server.close()),
  );

  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, CALLBACK_HOST, () => {
      const address = server.address();
      if (address && typeof address === "object") {
        callbackState.redirectUri = `http://${CALLBACK_HOST}:${address.port}${CALLBACK_PATH}`;
      }
      resolve();
    });
  });
  server.on("error", (error) => callbackState.reject?.(error));

  const [, code] = await Promise.all([
    onUrl(authorizeUrl(callbackState.redirectUri, challenge, state)),
    codePromise,
  ]);
  return exchangeCode({
    code,
    redirectUri: callbackState.redirectUri,
    verifier,
    ...(post ? { post } : {}),
  });
}
