// This file runs with bun.
import { request as httpRequest } from "node:http";
import {
  connect,
  type ClientHttp2Session,
  type ClientHttp2Stream,
  type IncomingHttpHeaders,
  type SecureClientSessionOptions,
} from "node:http2";
import { request as httpsRequest } from "node:https";
import type { Readable } from "node:stream";
import { URL } from "node:url";
import { createGunzip } from "node:zlib";

import { userAgent } from "./constants.ts";

export interface TransportTlsOptions {
  ALPNProtocols: string[];
  minVersion: "TLSv1.2";
  ciphers: string;
  sigalgs: string;
  ecdhCurve: string;
}

export interface Http2Settings {
  enablePush: boolean;
  initialWindowSize: number;
  maxFrameSize: number;
}

export interface HttpPostOptions {
  url: string;
  headers: Record<string, string>;
  body: string;
  signal?: AbortSignal;
}

export interface WireRequest {
  /** Absolute URL, e.g. `https://daily-cloudcode-pa.googleapis.com/v1internal:…`. */
  url: string;
  token: string;
  body: string;
  /** `text/event-stream` for SSE calls, `application/json` otherwise. */
  accept: "text/event-stream" | "application/json";
  signal?: AbortSignal;
  /** Optional dialer override for offline tests. */
  dial?: (origin: string, options: SecureClientSessionOptions) => ClientHttp2Session;
  /** Optional HTTP/1.1 fallback override for offline tests. */
  fallback?: (options: HttpPostOptions) => Promise<WireResponse>;
}

export interface WireResponse {
  status: number;
  headers: IncomingHttpHeaders;
  /** Body chunks; gzip is transparently inflated. */
  body: AsyncIterable<Buffer>;
  /** Consume the whole body as UTF-8 text. */
  text(): Promise<string>;
}

interface SettledState {
  settled: boolean;
}

interface Http2Context {
  state: SettledState;
  origin: string;
  client: ClientHttp2Session;
  stream: ClientHttp2Stream;
  onClientError: () => void;
  onAbort: () => void;
  signal?: AbortSignal;
  resolve: (value: WireResponse) => void;
  reject: (error: Error) => void;
}

/** Go's default TLS 1.2 cipher suites in Go's preference order (OpenSSL names). */
const GO_CIPHER_ORDER = [
  "ECDHE-ECDSA-AES128-GCM-SHA256",
  "ECDHE-RSA-AES128-GCM-SHA256",
  "ECDHE-ECDSA-CHACHA20-POLY1305",
  "ECDHE-RSA-CHACHA20-POLY1305",
  "ECDHE-ECDSA-AES256-GCM-SHA384",
  "ECDHE-RSA-AES256-GCM-SHA384",
  "ECDHE-RSA-AES128-SHA",
  "ECDHE-RSA-AES256-SHA",
  "ECDHE-ECDSA-AES128-SHA",
  "ECDHE-ECDSA-AES256-SHA",
  // No `RSA-PSS-RSAE-SHA256` here: Go has no such suite, and BoringSSL (Bun) rejects the name.
  "AES128-GCM-SHA256",
  "AES256-GCM-SHA384",
  "AES128-SHA",
  "AES256-SHA",
].join(":");

/** Go's client signature algorithms, most preferred first. */
const GO_SIGNATURE_ALGORITHMS = [
  "ecdsa_secp256r1_sha256",
  "rsa_pss_rsae_sha256",
  "rsa_pkcs1_sha256",
  "ecdsa_secp384r1_sha384",
  "rsa_pss_rsae_sha384",
  "rsa_pkcs1_sha384",
  "rsa_pss_rsae_sha512",
  "rsa_pkcs1_sha512",
  "rsa_pkcs1_sha1",
].join(":");

/** Go's http2 client SETTINGS: 16 KiB frames, 4 MiB stream flow window. */
const GO_HTTP2_SETTINGS: Http2Settings = {
  enablePush: false,
  initialWindowSize: 4 * 1024 * 1024,
  maxFrameSize: 16 * 1024,
};

export const TRANSPORT_TLS_OPTIONS: TransportTlsOptions = {
  ALPNProtocols: ["h2"],
  minVersion: "TLSv1.2",
  ciphers: GO_CIPHER_ORDER,
  sigalgs: GO_SIGNATURE_ALGORITHMS,
  ecdhCurve: "X25519:P-256:P-384",
};

const IDLE_TIMEOUT_MS = 60_000;
const REQUEST_TIMEOUT_MS = 120_000;
const POOLS: Map<string, ClientHttp2Session> = new Map();

/** Header map in the order the CLI's Go client writes them. */
export function wireHeaders(request: WireRequest, byteLength?: number): Record<string, string> {
  return {
    authorization: `Bearer ${request.token}`,
    "content-type": "application/json",
    accept: request.accept,
    "accept-encoding": "gzip",
    "user-agent": userAgent(),
    "content-length": String(byteLength ?? Buffer.byteLength(request.body)),
  };
}

function decode(stream: Readable, headers: IncomingHttpHeaders): AsyncIterable<Buffer> {
  const encoding = String(headers["content-encoding"] ?? "").toLowerCase();
  if (!encoding.includes("gzip")) {
    return stream;
  }
  const gunzip = createGunzip();
  stream.on("error", (error) => gunzip.destroy(error));
  stream.pipe(gunzip);
  return gunzip;
}

function dropFromPool(origin: string, client: ClientHttp2Session): void {
  if (POOLS.get(origin) === client) {
    POOLS.delete(origin);
  }
}

function pooledClient(
  origin: string,
  dial: (targetOrigin: string, options: SecureClientSessionOptions) => ClientHttp2Session,
): ClientHttp2Session {
  const existing = POOLS.get(origin);
  if (existing && !existing.closed && !existing.destroyed) {
    return existing;
  }
  const client = dial(origin, {
    ...TRANSPORT_TLS_OPTIONS,
    settings: { ...GO_HTTP2_SETTINGS },
  });
  client.setTimeout(IDLE_TIMEOUT_MS, () => {
    dropFromPool(origin, client);
    client.close();
  });
  client.on("error", () => dropFromPool(origin, client));
  client.on("goaway", () => dropFromPool(origin, client));
  client.on("close", () => dropFromPool(origin, client));
  POOLS.set(origin, client);
  return client;
}

/** Close pooled connections (session shutdown, tests). */
export function closePool(): void {
  for (const [origin, client] of POOLS) {
    POOLS.delete(origin);
    if (!client.destroyed) {
      client.close();
    }
  }
}

async function* emptyBody(): AsyncIterable<Buffer> {
  // No body (e.g. HEAD or an aborted stream).
}

async function readAllText(body: AsyncIterable<Buffer>): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of body) {
    chunks.push(chunk);
  }
  return Buffer.concat(chunks).toString("utf8");
}

const toWireResponse = (
  status: number,
  headers: IncomingHttpHeaders,
  body: AsyncIterable<Buffer>,
): WireResponse => ({
  status,
  headers,
  body,
  text: () => readAllText(body),
});

/** Standard Node HTTP/HTTPS POST used for OAuth and HTTP/2 fallback without global `fetch`. */
export function httpPost(options: HttpPostOptions): Promise<WireResponse> {
  const url = new URL(options.url);
  const send = url.protocol === "http:" ? httpRequest : httpsRequest;
  return new Promise<WireResponse>((resolve, reject) => {
    const req = send(
      url,
      {
        method: "POST",
        headers: options.headers,
        ...(options.signal ? { signal: options.signal } : {}),
      },
      (res) => resolve(toWireResponse(res.statusCode ?? 0, res.headers, decode(res, res.headers))),
    );
    req.on("error", reject);
    req.end(options.body);
  });
}

function fallbackHttp(request: WireRequest): Promise<WireResponse> {
  const headers = wireHeaders(request);
  const post = request.fallback ?? httpPost;
  return post({
    url: request.url,
    headers: { ...headers, "accept-encoding": "identity" },
    body: request.body,
    ...(request.signal ? { signal: request.signal } : {}),
  });
}

function connectSession(
  origin: string,
  dial: (targetOrigin: string, options: SecureClientSessionOptions) => ClientHttp2Session,
): ClientHttp2Session | undefined {
  try {
    return pooledClient(origin, dial);
  } catch (error) {
    console.warn(
      `my-antigravity: HTTP/2 dial failed, falling back to HTTP/1.1: ${error instanceof Error ? error.message : String(error)}`,
    );
    return undefined;
  }
}

const toError = (error: unknown): Error =>
  error instanceof Error ? error : new Error(String(error));

function markSettled(context: Http2Context): boolean {
  if (context.state.settled) {
    return false;
  }
  context.state.settled = true;
  context.client.off("error", context.onClientError);
  context.signal?.removeEventListener("abort", context.onAbort);
  return true;
}

const streamErrorHandler =
  (context: Http2Context) =>
  (error: unknown): void => {
    if (!markSettled(context)) {
      return;
    }
    dropFromPool(context.origin, context.client);
    context.client.destroy();
    context.reject(toError(error));
  };

const streamResponseHandler =
  (context: Http2Context) =>
  (headers: IncomingHttpHeaders): void => {
    context.stream.setTimeout(0);
    if (!markSettled(context)) {
      return;
    }
    context.resolve(
      toWireResponse(Number(headers[":status"] ?? 0), headers, decode(context.stream, headers)),
    );
  };

const streamCloseHandler = (context: Http2Context) => (): void => {
  if (!markSettled(context)) {
    return;
  }
  context.resolve(toWireResponse(0, {}, emptyBody()));
};

const streamTimeoutHandler = (context: Http2Context) => (): void => {
  if (!markSettled(context)) {
    return;
  }
  context.stream.close();
  dropFromPool(context.origin, context.client);
  context.client.destroy();
  context.reject(new Error("Antigravity request timed out waiting for server response"));
};

const streamAbortHandler = (context: Http2Context) => (): void => {
  if (!markSettled(context)) {
    return;
  }
  context.stream.close();
  context.reject(toError(context.signal?.reason));
};

function sendHttp2(
  client: ClientHttp2Session,
  url: URL,
  request: WireRequest,
): Promise<WireResponse> {
  return new Promise<WireResponse>((resolve, reject) => {
    const state: SettledState = { settled: false };
    const onClientError = (): void => {
      state.settled = true;
      dropFromPool(url.origin, client);
      void fallbackHttp(request).then(resolve, reject);
    };

    const bodyBuffer = Buffer.from(request.body);
    client.once("error", onClientError);
    const stream = client.request(
      {
        ":method": "POST",
        ":path": `${url.pathname}${url.search}`,
        ...wireHeaders(request, bodyBuffer.byteLength),
      },
      request.signal ? { signal: request.signal } : undefined,
    );
    const context: Http2Context = {
      state,
      origin: url.origin,
      client,
      stream,
      onClientError,
      onAbort: () => {},
      ...(request.signal ? { signal: request.signal } : {}),
      resolve,
      reject,
    };
    context.onAbort = streamAbortHandler(context);
    request.signal?.addEventListener("abort", context.onAbort, { once: true });
    // Only enforce header timeout before response headers arrive; clear it once streaming starts.
    stream.setTimeout(REQUEST_TIMEOUT_MS, streamTimeoutHandler(context));
    stream.once("error", streamErrorHandler(context));
    stream.once("response", streamResponseHandler(context));
    stream.once("close", streamCloseHandler(context));
    stream.end(bodyBuffer);
  });
}

/**
 * POST over HTTP/2 the way the CLI does. Falls back to HTTP/1.1 only when the
 * HTTP/2 connection itself cannot be established, so a proxy-only environment
 * keeps working.
 */
export async function postWire(request: WireRequest): Promise<WireResponse> {
  if (request.signal?.aborted) {
    throw request.signal.reason ?? new Error("Request aborted");
  }
  const url = new URL(request.url);
  const client = connectSession(url.origin, request.dial ?? connect);
  return client ? await sendHttp2(client, url, request) : await fallbackHttp(request);
}
