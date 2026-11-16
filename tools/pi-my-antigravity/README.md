# pi-my-antigravity

Antigravity (Google Cloud Code Assist) provider for Pi, built to behave like the
installed official CLI (`agy` 1.5.9) instead of approximating it. It uses the
subscription OAuth credential — no Gemini API key — and speaks the same
`v1internal` endpoints with the same agent envelope.

```
/login my-antigravity        # Google sign-in (PKCE, loopback callback on :51123)
/model my-antigravity/gemini-4-hydrogen
```

## What it sends

| Piece              | Value                                                                                                                                                                                                                                                  | Why                                                                                                   |
| ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------- |
| Host               | `https://daily-cloudcode-pa.googleapis.com`                                                                                                                                                                                                            | `agy` talks to the daily host first; production is the fallback                                       |
| Transport          | HTTP/2 (`ALPN: h2`), Go TLS 1.2 cipher/sigalg order, Go SETTINGS (`16 KiB` frame / `4 MiB` window), 1 pooled session per origin                                                                                                                        | `agy` is a Go binary; `fetch` picks Node/Bun ciphers, header order and compression                    |
| Headers            | `authorization`, `content-type`, `accept` (`text/event-stream` or `application/json`), `accept-encoding: gzip`, `user-agent`, `content-length`                                                                                                         | fixed Go-client header set and order                                                                  |
| User-Agent         | `antigravity/cli/<version> (aidev_client; os_type=<go-os>; arch=<go-arch>; purpose=agent; auth_method=consumer)`                                                                                                                                       | reads installed `agy --version` (fallback `1.5.9`); the backend rejects stale fingerprints            |
| Startup APIs       | `loadCodeAssist`, `fetchUserInfo`, `fetchAvailableModels`                                                                                                                                                                                              | mirrors `agy` startup calls and populates `model_enum`                                                |
| Model id           | runtime id, e.g. `gemini-4-hydrogen-medium`                                                                                                                                                                                                            | that is what `agy models` prints; the thinking level is a tier suffix                                 |
| Session & Envelope | deterministic `trajectory_id` / `sessionId` (from pi `sessionId` or opening user turn), `requestId: <trajectory>-<index>`, `last_step_index`, `last_execution_id` (when `index > 0`), `model_enum`, `requestType: "agent"`, `userAgent: "antigravity"` | preserves one CLI trajectory across turns and resumed pi sessions while letting pi own the transcript |
| Thinking           | `generationConfig.thinkingConfig{includeThoughts,thinkingBudget}`                                                                                                                                                                                      | mirrors `agy --effort`                                                                                |
| Tools              | `functionDeclarations[].parametersJsonSchema`                                                                                                                                                                                                          | JSON Schema is accepted; the legacy `parameters` field rejects `anyOf`/`const` in pi's tool schemas   |

### Deliberate deviation: the system prompt travels as the first user turn

`agy` puts its prompt in `request.systemInstruction`. The backend answers that
field with a generic `429 RESOURCE_EXHAUSTED` ("This model is unavailable right
now") when the text comes from a third-party harness — the same text in
`contents` is accepted (upstream: Rahularya01/pi-antigravity#137, still open in
0.14.0). This provider therefore sends no `systemInstruction` at all: the prompt
becomes the leading `user` turn, which is why a request here differs from the
CLI's own body in exactly one field placement. `tests/stream.test.ts` asserts the
field stays absent.

### Tool calls

`gemini-4-*` rejects replayed `functionCall` parts that lost their
`thoughtSignature`, so the signature streamed back with the call is stored on the
pi `ToolCall` and echoed on the next turn.

## Authentication

- `/login my-antigravity` runs Google OAuth PKCE using
  `MY_ANTIGRAVITY_CLIENT_ID` and `MY_ANTIGRAVITY_CLIENT_SECRET` (or
  `ANTIGRAVITY_CLIENT_ID` / `ANTIGRAVITY_CLIENT_SECRET`); no client id or secret
  is hardcoded in source files.
- Scopes: `aicode`, `cloud-platform`, `userinfo.*`, `cclog`,
  `experimentsandconfigs` — the set `agy` requests.
- Credentials land in pi's auth store (`~/.pi/agent/auth.json`, provider
  `my-antigravity`) and are refreshed by pi; `projectId` is filled from
  `loadCodeAssist`, defaulting to `aicode-consumers`.
- Overrides: `MY_ANTIGRAVITY_USER_AGENT` (client fingerprint), `MY_ANTIGRAVITY_PROJECT_ID`.

## Models

`src/families.ts` carries the `agy models` list as a static baseline (Gemini 4
Hydrogen/Argon, Gemini 3.8 Flash, Claude Opus/Sonnet 5.5, GPT-OSS 120B,
Orchestrator). `refreshModels` calls `fetchAvailableModels` — the endpoint behind
`agy models` — and appends runtime families this account additionally exposes, so
newly enabled models appear without a package release. `/my-antigravity.models`
prints the live list.

## Checks

```sh
bun run check     # oxfmt --check + oxlint + tsc --noEmit + vitest --coverage (offline; no network, no model calls)
```

`git diff --check` must stay clean. Nothing here writes credentials; the tests use
fixtures only.

## Known limits

- Not an official Google product; it uses the same public client and endpoints as
  the installed CLI, so a backend change can require a version bump in
  `src/constants.ts` (`CLI_VERSION`, `USER_AGENT`).
- Only chat/streaming is implemented — no image generation, web-fetch or search
  side tools from the CLI.
- The system-prompt placement is a workaround for a backend blocklist; if Google
  starts inspecting `contents` too, this package needs revisiting.
