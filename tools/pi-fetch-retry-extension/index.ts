// This TypeScript file is executed with Bun.
import { setTimeout as sleep } from "node:timers/promises";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

/**
 * A pooled connection the peer already closed. Cloudflare's edge drops an idle keep-alive socket
 * long before a client pool does, so the next request is written into a dead connection and fails
 * before any response arrives. Measured through the gateway in session 01a1fcbf: 14 such failures
 * (`other side closed (UND_ERR_SOCKET)`, `read ECONNRESET`), each one recovered by Pi's own retry
 * after `retry.baseDelayMs` (65 s here), which is why the turn showed an error and stalled.
 */
const STALE =
  /ECONNRESET|ECONNABORTED|ECONNREFUSED|ETIMEDOUT|EPIPE|UND_ERR_SOCKET|UND_ERR_CLOSED|UND_ERR_CONNECT_TIMEOUT|socket hang up|other side closed/i;

/** The wait before retry n: the socket is already gone, so a fresh connection is the whole fix. */
export const BACKOFF_MS: readonly number[] = [100, 500];

const MAX_CAUSE_DEPTH = 4;

function codeOf(error: Error): string {
  const code: unknown = (error as { readonly code?: unknown }).code;
  return typeof code === "string" ? code : "";
}

/** The AI SDK wraps a transport failure, so its code may sit on the cause and not on the error. */
export function isStaleConnection(error: unknown): boolean {
  let current: unknown = error;
  for (let depth = 0; depth < MAX_CAUSE_DEPTH; depth += 1) {
    if (!(current instanceof Error)) {
      return false;
    }
    if (STALE.test(`${current.name} ${current.message} ${codeOf(current)}`)) {
      return true;
    }
    current = current.cause;
  }
  return false;
}

/**
 * A body that is still readable can be sent again; a stream cannot, and neither can a Request that
 * already gave its body away. Every provider request Pi makes carries a JSON string.
 */
function replayable(input: unknown, init: RequestInit | undefined): boolean {
  const body: unknown = init?.body;
  if (body !== undefined && body !== null) {
    const resendable =
      typeof body === "string" ||
      body instanceof Uint8Array ||
      body instanceof ArrayBuffer ||
      body instanceof URLSearchParams ||
      body instanceof Blob ||
      body instanceof FormData;
    if (!resendable) {
      return false;
    }
  }
  return !(input instanceof Request);
}

export interface RetryOptions {
  readonly attempts?: number;
  readonly wait?: (ms: number) => Promise<void>;
}

const pause = async (ms: number): Promise<void> => {
  await sleep(ms);
};

/** Wraps a fetch so a transport failure that produced no response is retried at once. */
export function withRetry(inner: typeof fetch, options: RetryOptions = {}): typeof fetch {
  const attempts = options.attempts ?? BACKOFF_MS.length;
  const wait = options.wait ?? pause;
  return async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const again = replayable(input, init);
    for (let attempt = 0; ; attempt += 1) {
      try {
        return await inner(input, init);
      } catch (error) {
        const retry =
          again && attempt < attempts && init?.signal?.aborted !== true && isStaleConnection(error);
        if (!retry) {
          throw error;
        }
        await wait(BACKOFF_MS[Math.min(attempt, BACKOFF_MS.length - 1)] ?? 0);
      }
    }
  };
}

const installed = new WeakSet<object>();

/** Installs the wrapper once per holder, so a reload cannot stack retries. */
export function install(holder: { fetch: typeof fetch }, options: RetryOptions = {}): boolean {
  if (installed.has(holder)) {
    return false;
  }
  holder.fetch = withRetry(holder.fetch.bind(holder) as typeof fetch, options);
  installed.add(holder);
  return true;
}

export default function fetchRetry(_pi: ExtensionAPI): void {
  install(globalThis);
}
