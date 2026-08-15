# pi-claudex-provider

Bidirectional integration package for Pi and Claudex.

Direction A exposes Pi providers as a raw model gateway over an authenticated Unix socket. It calls provider `streamSimple` directly, so Pi's agent loop does not run. Direction B registers configured Claudex models as the Pi provider `claudex` and uses Pi's Anthropic Messages streaming implementation to call the adapter.

The Direction A gateway is disabled unless both environment variables are set:

- `CLAUDEX_PI_GATEWAY_SOCKET`: absolute Unix socket path inside a private runtime directory
- `CLAUDEX_PI_GATEWAY_TOKEN`: per-process authentication token

Direction B configuration:

- `CLAUDEX_ADAPTER_BASE_URL`: adapter URL; default `http://127.0.0.1:8318`
- `CLAUDEX_PROVIDER_CONFIG`: model catalog source; default `~/.config/claudex/providers.json`
- `ANTHROPIC_AUTH_TOKEN`: required when the adapter is not on loopback

Direction B sends `x-claudex-origin: pi-provider`. The adapter must reject this origin if the selected route would enter the Pi gateway again. Direction A also excludes the `claudex` provider from model listing and rejects it explicitly.

## Gateway protocol

The socket uses strict LF-delimited JSON, version `1`. Every client message includes the per-process token.

1. Client sends `hello`; server sends `ready`.
2. `list_models` returns available Pi models except provider `claudex`.
3. `request` carries `provider`, `modelId`, raw Anthropic `system` / `messages` / `tools`, safe sampling options, and `origin: "claudex"`.
4. Server emits compact `text_*`, `thinking_*`, and `toolcall_*` events. `done` or `error` includes the full authoritative Pi assistant message and is terminal.
5. `cancel` aborts the matching request. Multiple authenticated connections and multiplexed request IDs are supported.

### Consumer conformance

The compact lifecycle events are incremental transport, not a replacement for the terminal Pi message. A consumer must correlate every event by exact request `id` and treat `done.message` or `error.error` as authoritative.

| Event                                                | Incremental fields                                        | Consumer requirement                                                                                                                                                                                                      | Failure if ignored                                                                  |
| ---------------------------------------------------- | --------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| `start`                                              | `provider`, `model`, `api`                                | Retain resolved response identity when it differs from the requested model.                                                                                                                                               | Auto-routed responses report the wrong provider or model.                           |
| `text_start` / `text_delta` / `text_end`             | `index`; `delta` or final `content`                       | Preserve order and block index. Reconcile accumulated deltas with final content instead of silently accepting missing suffixes.                                                                                           | Empty, split, or partially streamed text is silently dropped or merged incorrectly. |
| `thinking_start` / `thinking_delta` / `thinking_end` | `index`; `delta` or final `content`                       | Preserve order and block index. Protocol v1 intentionally carries visible thinking only; a consumer may translate its SSE signature when follow-up replay is validated for the selected provider.                         | Thinking disappears, or unvalidated signature translation breaks a follow-up.       |
| `toolcall_start` / `toolcall_delta` / `toolcall_end` | `index`, call ID, name, argument delta or final arguments | Emit each call once and preserve exact IDs and arguments. Retain terminal-only `thoughtSignature` and `namespace` metadata for provider continuation.                                                                     | A tool is omitted or duplicated, or provider continuation fails.                    |
| `done`                                               | `reason`, full `message`                                  | Own successful termination for this request only. Preserve the exact stop reason, reconcile terminal content without duplicating streamed content or tools, and retain provider continuation metadata and complete usage. | Truncation is reported as success; terminal-only content and usage are lost.        |
| `error`                                              | `reason`, full `error` message                            | Own failed termination for this request only and retain the provider error details.                                                                                                                                       | The request fails without its actionable provider error.                            |
| `protocol_error`                                     | `message`                                                 | Fail the matching request; never complete another request on the same session or thread.                                                                                                                                  | One request can fail or complete an unrelated turn.                                 |

Terminal reconciliation must account for fields required by the selected provider that incremental events cannot fully represent: `thinkingSignature`, `redacted`, `textSignature`, tool-call `thoughtSignature` and `namespace`, `responseId`, `responseModel`, `deferred`, detailed stop reasons, and usage including cache and reasoning tokens. A provider-validated session or signature translation may satisfy continuation without adding opaque signatures to incremental events. If terminal content extends an already streamed prefix, emit only the missing suffix. If it conflicts with streamed content, fail visibly rather than replaying or dropping content silently.

The current Claudex adapter intentionally does not reconcile terminal-only content. Every supported Pi route emits complete deltas, and end-to-end tests have not observed content loss. Add prefix-aware terminal reconciliation before supporting a provider that can omit deltas; unconditional replay can duplicate text or tool calls.

Install after verification:

```bash
bun install --cwd tools/pi-claudex-provider
bun run --cwd tools/pi-claudex-provider check
pi install "$PWD/tools/pi-claudex-provider"
```

## Thinking display characteristics (measured)

Gateway protocol v1 forwards `thinking_start` / `thinking_delta` / `thinking_end`
the moment Pi emits them; there is no buffering on the socket path (verified with
live socket captures). Whether a TUI can show live reasoning therefore depends
entirely on what the upstream provider streams. Measured behavior by model
family (2026-08-15, pi-ai 0.84.2):

| Family                             | Pi provider(s)                               | Live incremental thinking | Notes                                                                                                                                                                                                                                                                                                                                                |
| ---------------------------------- | -------------------------------------------- | ------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Qwen (thinkingFormat `qwen`)       | `qwen-token-plan-individual`                 | Yes                       | 123 deltas / 7.6 s, median gap 51 ms on a small prompt.                                                                                                                                                                                                                                                                                              |
| xAI Grok                           | `xai`                                        | Yes                       | 34 deltas / 18 s; pauses only when the model pauses.                                                                                                                                                                                                                                                                                                 |
| OpenAI responses (GPT)             | `openai-codex`, `github-copilot`, `opencode` | No — end burst only       | Reasoning summaries are generated after reasoning completes; `reasoning_text` (raw CoT) is never streamed to API consumers. Example: `gpt-5.6-luna` streams ~2 summary deltas only after the whole reasoning phase.                                                                                                                                  |
| `gpt-5.3-codex-spark` specifically | `openai-codex` (ChatGPT backend only)        | No — none at all          | The ChatGPT backend rejects `reasoning.summary` for this model (HTTP 400 `unsupported_parameter`); with `summary` omitted the stream carries zero `reasoning_summary_*` events even though `reasoning_tokens` are consumed (957 on a small prompt). `done.message` also contains an empty thinking block. No other Pi provider offers this model id. |

Consequences for consumers:

- A "thought for N seconds with no visible deltas" experience is the expected
  upstream behavior for all OpenAI responses models, not a gateway defect.
- For `gpt-5.3-codex-spark` the consumer sees no thinking block at all. This
  cannot be fixed on the client side: the backend simply does not expose the
  data. Alternatives with live thinking are Qwen and xAI routes; OpenAI routes
  via `github-copilot` (`gpt-5.3-codex`, non-spark) at least surface a short
  summary at the end of the reasoning phase.
- Do not synthesize fake `thinking_delta` pacing to smooth the end burst: it
  misrepresents timing, pollutes replayed thinking blocks, and destroys latency
  forensics on the wire.

## Isolated verification

Pi records local packages relative to its normal `~/.pi/agent` directory. Copying
`settings.json` into a temporary `PI_CODING_AGENT_DIR` therefore changes what
those paths resolve to and can silently prevent the gateway extension from
loading. Before an isolated run, rewrite the local Claudex, Cursor, and Cline
Pass package entries in the copied settings to absolute repository paths. Keep
npm package entries unchanged. Hash `~/.pi/agent/settings.json` before and after
the run to verify that the isolated test did not modify the user's settings.
