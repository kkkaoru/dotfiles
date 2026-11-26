# pi-my-antigravity

Antigravity provider for Pi that runs the official
[`gemini-api` CLI](https://github.com/google-gemini/gemini-api-cli) instead of
speaking the Cloud Code Assist wire itself. One child process per completion:

```
gemini-api generate --body <interaction JSON> --stream=false --output-format json
```

The CLI owns credentials, model ids, the endpoint and the request shape; this
package owns the translation between pi's transcript and the Interactions API
(`POST /v1beta/interactions`).

```
/model my-antigravity/gemini-4-hydrogen
```

## Requirements

- `gemini-api` on `PATH`, at `~/.local/bin/gemini-api`, or named by
  `GEMINI_API_CLI`. Install it with
  `curl -fsSL https://raw.githubusercontent.com/google-gemini/gemini-api-cli/main/scripts/install.sh | bash`.
- Credentials for the CLI: `gemini-api configure` (stored config, OS keychain) or
  `GEMINI_API_KEY`. Pi passes `GEMINI_API_KEY` through when it is set and
  otherwise lets the CLI resolve its own credential.

A missing CLI is reported in the turn that needs it, not at load time, so the
provider stays harmless on a machine without the binary.

## What it sends

| Piece       | Value                                                                                        |
| ----------- | -------------------------------------------------------------------------------------------- |
| Prompt      | pi's replayed system messages, as the leading `user_input` step                              |
| Tools       | `{type: "function", name, description, parameters}` from the active pi tool set              |
| User turn   | `user_input` with `text` and `image` (`mime_type`, base64 `data`) content blocks             |
| Assistant   | `model_output` with `text` and `function_call` (`id`, `name`, `arguments`) steps             |
| Tool result | `function_result` with `call_id`, `name`, `result`, and `is_error` when the tool failed      |
| Usage       | `usage.input_tokens` / `cached_tokens` / `output_tokens`, cached tokens moved to `cacheRead` |

Non-streamed: the CLI is asked for one JSON interaction, which is then replayed
to pi as `start` → `thinking_*` / `text_*` / `toolcall_*` → `done` events, so pi
sees the same event stream it gets from a streaming provider.

## Models

`gemini-4-hydrogen` is the static entry. `refreshModels` asks
`gemini-api models list --output-format json` and merges whatever the account
reports; a failure or an empty answer keeps the static catalog. The per-token
price is zero because the CLI bills through the account, not per request.

## Checks

```
bun run check   # oxfmt, oxlint (type-aware), tsc, vitest with 95% coverage
```
