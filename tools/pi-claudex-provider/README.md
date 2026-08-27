# pi-claudex-provider

Bidirectional integration package for Pi and Claudex.

Direction A exposes Pi providers as a raw model gateway over an authenticated Unix socket. It calls provider `streamSimple` directly, so Pi's agent loop does not run. Direction B registers configured Claudex models as the Pi provider `claudex` and uses Pi's Anthropic Messages streaming implementation to call the adapter.

Because Direction A does not run Pi's agent loop, Pi `tool_call` extensions cannot intercept the
Claude Code tools returned by a routed model. This package therefore imports the long-command policy
from `pi-tmux-timeout-extension`, adds the Claude-native background lifecycle guidance to the Bash
tool description, and normalizes matching Bash calls to `run_in_background=true`. It prefixes the
background description with execution-local `MM-DD HH:mm`, while Claude Code owns the task id, output path, user-input
availability, and completion notification. Claudex's isolated `PreToolUse` hook applies the same
policy again at the Claude Code execution boundary as a fallback. The adapter treats non-Agent task
notifications as actionable continuation turns rather than swallowing them as SubAgent lifecycle
noise. Completion never stops the originating SubAgent: user follow-ups use `SendMessage`, while the
Claudex Bash `PostToolUse` `asyncRewake` hook delivers completion metadata directly to the same
originating context. A parent-side notification uses `SendMessage` only as a fallback, and only that
SubAgent or the user decides when it finishes.

The Direction A gateway is disabled unless both environment variables are set:

- `CLAUDEX_PI_GATEWAY_SOCKET`: absolute Unix socket path inside a private runtime directory
- `CLAUDEX_PI_GATEWAY_TOKEN`: per-process authentication token

Direction B configuration:

- `CLAUDEX_ADAPTER_BASE_URL`: adapter URL; default `http://127.0.0.1:8318`
- `CLAUDEX_PROVIDER_CONFIG`: model catalog source; default `~/.config/claudex/providers.json`
- `ANTHROPIC_AUTH_TOKEN`: required when the adapter is not on loopback

Loopback Direction B uses the adapter launcher's canonical `claudex-local` token. Discovered Claudex
models force Anthropic adaptive thinking so Pi's displayed thinking level is serialized as
`output_config.effort`; this keeps Pi's dynamic-effort TUI value identical to the adapter's logged
`request_effort`.

Direction B sends `x-claudex-origin: pi-provider`. The adapter must reject this origin if the selected route would enter the Pi gateway again. Direction A also excludes the `claudex` provider from model listing and rejects it explicitly.

## Gateway protocol

The socket uses strict LF-delimited JSON, version `1`. Every client message includes the per-process token.

1. Client sends `hello`; server sends `ready`.
2. `list_models` returns available Pi models except provider `claudex`.
3. `request` carries `provider`, `modelId`, raw Anthropic `system` / `messages` / `tools`, safe sampling options, and `origin: "claudex"`.
4. Server emits compact `text_*`, normalized `thinking_start` / `thinking_progress` / `thinking_result`, and `toolcall_*` events. `done` or `error` includes the full authoritative Pi assistant message and is terminal. A `done` event also includes the provider-neutral `terminal` contract described below.
5. `cancel` aborts the matching request. Multiple authenticated connections and multiplexed request IDs are supported.
6. `web_search` carries `provider`, `modelId`, and `query`. The server replies with `web_search_result` (`results: [{title,url,snippet}]`, possibly empty) or `web_search_error` (`provider`, `modelId`, `message`).

### Web search (`delegate-pi`)

Adapter `webSearchMode=delegate-pi` reaches this socket via `/worker/web-search`.

| Session provider                 | Search path                                          | Notes                                                             |
| -------------------------------- | ---------------------------------------------------- | ----------------------------------------------------------------- |
| `cursor`                         | `modelRegistry.complete()` prompt-only native search | Same-session model only; no silent Exa fallback                   |
| any other non-`claudex` provider | Exa `POST https://api.exa.ai/search`                 | Requires `EXA_API_KEY`; missing key → explicit `web_search_error` |
| `claudex`                        | rejected                                             | Avoids recursive gateway search                                   |

Empty Exa/Cursor result sets are success (`results: []`), not errors. Cross-provider fallback is forbidden.

**Daily TUI usage does not exercise this path.** The orchestrator delegates search to `claudex-haiku-search` (`claude-haiku-4-5`), which is forced onto the Claude Subscription route and never enters Pi. Cursor main turns may also search inside the model without emitting `WebSearch`. Keep public routes on CCR / provider-native search; treat HTTP/socket GREEN as transport proof, not daily-path proof. See [`.config/claudex/README.md`](../../.config/claudex/README.md) (“Claude models stay off the Pi gateway” / “WebSearchの経路”).

### Claude models are not Pi routes

Native Claude models (`claude-*`, `opus` / `sonnet` / `haiku` / `fable`, and `[1m]` aliases) must stay on Claude Subscription. They must not appear as Pi gateway targets. Discovery ids under `claude-claudex-*` are excluded from that Claude-native check. Policy and adapter guards live in Claudex config docs, not in this package.

### Consumer conformance

The compact lifecycle events are incremental transport, not a replacement for the terminal Pi message. A consumer must correlate every event by exact request `id` and treat `done.message` or `error.error` as authoritative.

`done.terminal` normalizes model-specific termination behavior. `output` is `assistant`, `tool_use`, or `none`. `state` is `complete` when the authoritative message contains usable assistant text or a tool call, and `recoverable_error` when `stop`/`length` is empty or `toolUse` contains no tool call. Recoverable codes are `empty_assistant` and `tool_use_without_call`. Consumers may apply stricter tool-schema validation; if a declared tool call cannot be forwarded, they must downgrade the turn to the same recoverable empty-output path.

| Event                                                      | Incremental fields                                        | Consumer requirement                                                                                                                                                                                                                                                               | Failure if ignored                                                                           |
| ---------------------------------------------------------- | --------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| `start`                                                    | `provider`, `model`, `api`                                | Retain resolved response identity when it differs from the requested model.                                                                                                                                                                                                        | Auto-routed responses report the wrong provider or model.                                    |
| `text_start` / `text_delta` / `text_end`                   | `index`; `delta` or final `content`                       | Preserve order and block index. Reconcile accumulated deltas with final content instead of silently accepting missing suffixes.                                                                                                                                                    | Empty, split, or partially streamed text is silently dropped or merged incorrectly.          |
| `thinking_start` / `thinking_progress` / `thinking_result` | `index`; final `result` only                              | Treat start and contentless progress as live activity. Display only the bounded terminal result; raw chain-of-thought is intentionally absent.                                                                                                                                     | Thought activity disappears, or private reasoning is exposed.                                |
| `toolcall_start` / `toolcall_delta` / `toolcall_end`       | `index`, call ID, name, argument delta or final arguments | Emit each call once and preserve exact IDs and arguments. Retain terminal-only `thoughtSignature` and `namespace` metadata for provider continuation.                                                                                                                              | A tool is omitted or duplicated, or provider continuation fails.                             |
| `done`                                                     | `reason`, full `message`, `terminal`                      | Own termination for this request only. Preserve the exact stop reason, reconcile terminal content without duplicating streamed content or tools, and retain provider continuation metadata and complete usage. `terminal.state=recoverable_error` must not be reported as success. | Truncation or empty output is reported as success; terminal-only content and usage are lost. |
| `error`                                                    | `reason`, full `error` message                            | Own failed termination for this request only and retain the provider error details.                                                                                                                                                                                                | The request fails without its actionable provider error.                                     |
| `protocol_error`                                           | `message`                                                 | Fail the matching request; never complete another request on the same session or thread.                                                                                                                                                                                           | One request can fail or complete an unrelated turn.                                          |

Terminal reconciliation must account for fields required by the selected provider that incremental events cannot fully represent: `thinkingSignature`, `redacted`, `textSignature`, tool-call `thoughtSignature` and `namespace`, `responseId`, `responseModel`, `deferred`, detailed stop reasons, and usage including cache and reasoning tokens. A provider-validated session or signature translation may satisfy continuation without adding opaque signatures to incremental events. If terminal content extends an already streamed prefix, emit only the missing suffix. If it conflicts with streamed content, fail visibly rather than replaying or dropping content silently.

The current Claudex adapter intentionally does not reconcile terminal-only content. Every supported Pi route emits complete deltas, and end-to-end tests have not observed content loss. Add prefix-aware terminal reconciliation before supporting a provider that can omit deltas; unconditional replay can duplicate text or tool calls.

Install after verification:

```bash
bun install --cwd tools/pi-claudex-provider
bun run --cwd tools/pi-claudex-provider check
pi install "$PWD/tools/pi-claudex-provider"
```

## Thinking display characteristics (measured)

Gateway protocol v1 normalizes every Pi thinking lifecycle into a model-neutral
contract. `thinking_start` opens activity, every upstream delta immediately emits
a contentless `thinking_progress`, and `thinking_result` carries only a bounded
result when the thought ends. Raw chain-of-thought is never placed in incremental
Thought events. The authoritative `done.message` may still retain provider-owned
thinking/signatures for validated continuation, but consumers must not render it.
For Responses APIs the provider extracts the native terminal reasoning summary
from Pi's signed reasoning item. Other APIs use the final non-empty paragraph of
the terminal thinking block. Redacted thinking yields an empty result, and every
non-empty result is limited to 400 grapheme clusters.

The upstream characteristics that feed this normalization were measured by model
family (2026-08-15, pi-ai 0.84.2):

| Family                             | Pi provider(s)                               | Live incremental thinking | Notes                                                                                                                                                                                                                                                                                                                                                |
| ---------------------------------- | -------------------------------------------- | ------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Qwen (thinkingFormat `qwen`)       | `qwen-token-plan-individual`                 | Yes                       | 123 deltas / 7.6 s, median gap 51 ms on a small prompt.                                                                                                                                                                                                                                                                                              |
| xAI Grok                           | `xai`                                        | Yes                       | 34 deltas / 18 s; pauses only when the model pauses.                                                                                                                                                                                                                                                                                                 |
| OpenAI responses (GPT)             | `openai-codex`, `github-copilot`, `opencode` | No — end burst only       | Reasoning summaries are generated after reasoning completes; `reasoning_text` (raw CoT) is never streamed to API consumers. Example: `gpt-5.6-luna` streams ~2 summary deltas only after the whole reasoning phase.                                                                                                                                  |
| `gpt-5.3-codex-spark` specifically | `openai-codex` (ChatGPT backend only)        | No — none at all          | The ChatGPT backend rejects `reasoning.summary` for this model (HTTP 400 `unsupported_parameter`); with `summary` omitted the stream carries zero `reasoning_summary_*` events even though `reasoning_tokens` are consumed (957 on a small prompt). `done.message` also contains an empty thinking block. No other Pi provider offers this model id. |

Consequences for consumers:

- OpenAI Responses models may provide no intermediate progress because their
  reasoning summary is generated only at the end; the terminal summary is still
  returned as `thinking_result` when the backend exposes one.
- For `gpt-5.3-codex-spark` the consumer sees no thinking block at all. This
  cannot be fixed on the client side: the backend simply does not expose the
  data. Alternatives with live thinking are Qwen and xAI routes; OpenAI routes
  via `github-copilot` (`gpt-5.3-codex`, non-spark) at least surface a short
  summary at the end of the reasoning phase.
- The gateway does not synthesize fake pacing. `thinking_progress` corresponds
  one-for-one with actual upstream delta arrival and deliberately omits content.

## Isolated verification

Pi records local packages relative to its normal `~/.pi/agent` directory. Copying
`settings.json` into a temporary `PI_CODING_AGENT_DIR` therefore changes what
those paths resolve to and can silently prevent the gateway extension from
loading. Before an isolated run, rewrite the local Claudex, Cursor, and Cline
Pass package entries in the copied settings to absolute repository paths. Keep
npm package entries unchanged. Hash `~/.pi/agent/settings.json` before and after
the run to verify that the isolated test did not modify the user's settings.
