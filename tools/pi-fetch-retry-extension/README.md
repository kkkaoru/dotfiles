# pi-fetch-retry

Retries a provider request that failed on a connection the peer had already closed, instead of
letting the turn end with `Error: Connection error.`.

## Why

Cloudflare's edge drops an idle keep-alive socket long before a client pool does. The next request
is then written into a dead connection and fails before any response arrives:

```
Connection error.: other side closed (UND_ERR_SOCKET)
Connection error.: read ECONNRESET
```

Pi already treats these as retryable (`isRetryableAssistantError`), so no turn was ever lost — but
the retry waits `retry.baseDelayMs` (65 s in this setup) and the error is shown first. In session
`01a1fcbf` that was 14 failures, each one recovered a minute later.

Lowering `retry.baseDelayMs` would fix the socket case and break the 429 case, where a one-minute
rate limit needs the long wait. This extension keeps the two apart: a dead socket is retried in
100 ms at the transport layer, and the agent-level backoff stays as it is for throttling.

## What it does

`globalThis.fetch` is wrapped once, at extension load, which is the `fetch` the provider client
reads when it is constructed. A request is retried only when all of these hold:

- it failed before any response arrived, so nothing has been streamed;
- the failure is a transport failure (`ECONNRESET`, `UND_ERR_SOCKET`, `other side closed`,
  `socket hang up`, …), including one wrapped in a `cause`;
- the body can be sent again — a string, bytes, `URLSearchParams`, `Blob` or `FormData`, not a
  stream or an already-sent `Request`;
- the caller has not aborted.

Two retries, 100 ms and 500 ms apart. A response that has started streaming is never touched.

## Check

```sh
bun install
bun run check
```
