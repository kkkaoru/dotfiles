// This file runs with bun.
import {
  connect,
  type ClientHttp2Session,
  type IncomingHttpHeaders,
  type SecureClientSessionOptions,
} from "node:http2";
import type { Readable } from "node:stream";
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
}

export interface WireResponse {
  status: number;
  headers: IncomingHttpHeaders;
  /** Body chunks; gzip is transparently inflated. */
  body: AsyncIterable<Buffer>;
  /** Consume the whole body as UTF-8 text. */
  text(): Promise<string>;
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
  "RSA-PSS-RSAE-SHA256",
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
const POOLS: Map<string, ClientHttp2Session> = new Map();

/** Header map in the order the CLI's Go client writes them. */
export function wireHeaders(request: WireRequest): Record<string, string> {
  return {
    authorization: `Bearer ${request.token}`,
    "content-type": "application/json",
    accept: request.accept,
    "accept-encoding": "gzip",
    "user-agent": userAgent(),
    "content-length": String(Buffer.byteLength(request.body)),
  };
}

async function* decode(stream: Readable, headers: IncomingHttpHeaders): AsyncIterable<Buffer> {
  const encoding = String(headers["content-encoding"] ?? "").toLowerCase();
  if (!encoding.includes("gzip")) {
    for await (const chunk of stream) {
      yield chunk as Buffer;
    }
    return;
  }
  const gunzip = createGunzip();
  stream.on("error", (error) => gunzip.destroy(error));
  stream.pipe(gunzip);
  for await (const chunk of gunzip) {
    yield chunk as Buffer;
  }
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

function fallbackFetch(request: WireRequest): Promise<WireResponse> {
  const headers = wireHeaders(request);
  return fetch(request.url, {
    method: "POST",
    headers: { ...headers, "accept-encoding": "identity" },
    body: request.body,
    ...(request.signal ? { signal: request.signal } : {}),
  }).then(async (response) => {
    const buffer = Buffer.from(await response.arrayBuffer());
    const body: AsyncIterable<Buffer> = (async function* () {
      if (buffer.length > 0) {
        yield buffer;
      }
    })();
    return {
      status: response.status,
      headers: Object.fromEntries(response.headers.entries()),
      body,
      text: async () => buffer.toString("utf8"),
    };
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
      `my-antigravity: HTTP/2 dial failed, falling back to fetch: ${error instanceof Error ? error.message : String(error)}`,
    );
    return undefined;
  }
}

/**
 * POST over HTTP/2 the way the CLI does. Falls back to `fetch` only when the
 * HTTP/2 connection itself cannot be established, so a proxy-only environment
 * keeps working.
 */
export async function postWire(request: WireRequest): Promise<WireResponse> {
  if (request.signal?.aborted) {
    throw request.signal.reason ?? new Error("Request aborted");
  }
  const url = new URL(request.url);
  const client = connectSession(url.origin, request.dial ?? connect);
  if (!client) {
    return fallbackFetch(request);
  }

  return await new Promise<WireResponse>((resolve, reject) => {
    const state = { settled: false };
    const cleanup = (): void => {
      client.off("error", onClientError);
    };
    const fail = (error: unknown): void => {
      if (state.settled) {
        return;
      }
      state.settled = true;
      cleanup();
      dropFromPool(url.origin, client);
      client.destroy();
      reject(error instanceof Error ? error : new Error(String(error)));
    };
    const onClientError = (error: unknown): void => {
      if (!state.settled) {
        state.settled = true;
        cleanup();
        dropFromPool(url.origin, client);
        void fallbackFetch(request).then(resolve, reject);
        return;
      }
      fail(error);
    };

    client.once("error", onClientError);

    const stream = client.request(
      {
        ":method": "POST",
        ":path": `${url.pathname}${url.search}`,
        ...wireHeaders(request),
      },
      request.signal ? { signal: request.signal } : undefined,
    );
    stream.on("error", fail);
    stream.on("response", (headers) => {
      if (state.settled) {
        return;
      }
      state.settled = true;
      cleanup();
      const body = decode(stream, headers);
      resolve({
        status: Number(headers[":status"] ?? 0),
        headers,
        body,
        text: async () => {
          const chunks: Buffer[] = [];
          for await (const chunk of body) {
            chunks.push(chunk);
          }
          return Buffer.concat(chunks).toString("utf8");
        },
      });
    });
    stream.end(request.body);
    stream.on("close", () => {
      if (!state.settled) {
        state.settled = true;
        cleanup();
        resolve({ status: 0, headers: {}, body: emptyBody(), text: async () => "" });
      }
    });
  });
}
