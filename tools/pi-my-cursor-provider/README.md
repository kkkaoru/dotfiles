# pi-my-cursor-provider

Minimal Cursor agent bridge for pi and Claudex. It preserves the existing `cursor/auto` model identity while using Cursor's agent SDK rather than pretending Cursor exposes a raw inference API.

## Guarantees

- Creates a fresh local Cursor agent for each independent request; it never calls `Agent.resume()`.
- Serializes independent Cursor requests so a new `send()` waits until the previous local agent has been cancelled and disposed. This avoids Cursor's `AgentBusyError` after abort or a follow-up prompt.
- Keeps a live Cursor run only across the tool-result continuation belonging to that same request.
- Converts pi `Context.tools` schemas into Cursor SDK `customTools`.
- Returns Cursor custom-tool callbacks as normal pi tool calls and resolves them from the next `ToolResultMessage` context.
- Leaves Cursor-native tools enabled.
- Discovers the authenticated Cursor model catalog and provides a multi-model fallback catalog when discovery is unavailable.
- Uses pi's supplied system prompt and tools without reloading ambient Cursor setting sources, avoiding duplicate rules and SDK bootstrap logs in standalone TUI use.
- Advertises Cursor models to pi at 80% of their real context window (256k models report 204.8k) so pi's native auto-compaction fires before requests can reach Cursor's hard limit, where Cursor returns usage-guideline blocks instead of recognizable overflow errors.
- Compaction summaries are routed through an off-Cursor fallback chain — `ollama-cloud/kimi-k3` → `github-copilot/gemini-3.7-flash` → `commandcode/gemini-3.7-flash` — because Cursor's moderation frequently blocks pi's whole-conversation summarization payloads. Each candidate is checked for configured auth and retried down the chain on failure; when the whole chain is unavailable, pi's default compaction runs instead.

A Cursor API key must be available through pi `/login`, `CURSOR_API_KEY`, or request-level `--api-key` resolution.

## Models

Use `/model` in the TUI or select a model directly:

```bash
pi --model cursor/auto
pi --model cursor/composer-2.5
pi --model cursor/claude-sonnet-4-6
pi --model cursor/gpt-5.6-sol
pi --model cursor/gemini-3.1-pro
```

The fallback catalog also includes Opus, GPT-5.4, Grok, Kimi, and GLM models. Pi model refresh uses `Cursor.models.list()` with the configured Cursor credential to replace it with the account's current catalog.

## Request fidelity

Behavior with `@cursor/sdk` 1.0.28:

| Input                                             | Behavior                                                                                                                                                                                                                                                                                |
| ------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Model ID, Cursor credential, abort signal         | Passed to the Cursor agent.                                                                                                                                                                                                                                                             |
| Pi tools                                          | Converted to Cursor custom tools with names, descriptions, and JSON schemas; results return to the same live run.                                                                                                                                                                       |
| System prompt and message history                 | Transformed into one role-labelled transcript because each independent request uses a fresh Cursor agent.                                                                                                                                                                               |
| Images                                            | Every user and tool-result image in the history is attached again in transcript order. Numbered placeholders such as `[image 2 attached: image/jpeg]` associate transcript positions with the SDK image array. Duplicate images are preserved.                                          |
| Reasoning level                                   | Mapped to the live catalog's `effort` or `reasoning` parameter for explicitly selected capable models. Cursor Auto and models without such a parameter emit a warning and retain their SDK defaults.                                                                                    |
| Maximum output tokens and temperature             | Not configurable through the Cursor Agent SDK. Pi model values are descriptive metadata only.                                                                                                                                                                                           |
| Session ID, cache retention, and request metadata | Not configurable through the Cursor Agent SDK. Independent requests intentionally create fresh agents.                                                                                                                                                                                  |
| Compaction summarization model                    | Never Cursor. `session_before_compact` intercepts compaction for cursor models and summarizes via the off-Cursor chain `ollama-cloud/kimi-k3` → `github-copilot/gemini-3.7-flash` → `commandcode/gemini-3.7-flash`, deferring to pi's default compaction when no chain model is usable. |

The SDK publishes no image count or byte limit, so the provider never silently truncates image history. An upstream limit is returned as an explicit request error. Reattaching all earlier images on every independent turn preserves correctness but increases payload size and latency in image-heavy conversations.

## Development

```bash
bun install
bun run check
pi --approve -e . --model cursor/auto
```

Run unit tests with `bun --cwd tools/pi-my-cursor-provider test` (vitest). A bare `bun test` in this directory uses bun's runner and fails on `importOriginal`.

### Manual TUI tool roundtrip

`tests/tui-probe-extension.ts` is a manual fixture for verifying that a Cursor custom-tool callback crosses the provider bridge, is executed by pi, and returns its result to the same Cursor run. Start both extensions:

```bash
pi --approve \
  -e . \
  -e ./tests/tui-probe-extension.ts \
  --model cursor/auto \
  --no-session
```

Ask Cursor to call `cursor_bridge_probe`. A successful roundtrip displays the tool result `CURSOR_BRIDGE_TOOL_OK` before Cursor completes its response.

## Web Search via delegate-pi

The Claudex adapter supports a `delegate-pi` web search mode (commits 619af02, ff5a12f) that routes Claude Code's WebSearch tool through the Pi gateway instead of CCR workers. The gateway's `cursorWebSearch` handler uses `modelRegistry.complete()` with a structured-output prompt to obtain Title/URL/Snippet triplets from Cursor's native server-side search.

### Current status

- **HTTP-level verification**: GREEN. Cursor returns 5 structured results in ~15s; Exa API (non-Cursor providers) returns 5 results in ~1.4s.
- **TUI practical usage**: The orchestrator model delegates search to the `claudex-haiku-search` SubAgent (a nativeWorker using `claude-haiku-4-5` on the Subscription route), which bypasses the Pi gateway entirely. The delegate-pi path is therefore not exercised during normal interactive use.
- **CCR fallback**: The existing CCR pin (commit 79447b7) remains active and functional. Users are unaffected.

### Why delegate-pi is not reached in practice

1. Claude Code's orchestrator prefers delegating WebSearch to `claudex-haiku-search` (nativeWorker, Subscription route)
2. `claudex-haiku-search` is defined in `nativeWorkers`, NOT in `providers` — it has no PiGateway route
3. Even when delegate-pi is enabled for the cursor route, the main model's SubAgent delegation takes precedence

### Future activation paths

- Remove `claudex-haiku-search` from nativeWorkers so the orchestrator uses WebSearch directly
- Add an Anthropic provider to Pi and route haiku-search through PiGateway
- Modify adapter request routing to intercept WebSearch before SubAgent delegation

## Replace `pi-cursor-sdk`

The current installation is an npm package recorded as `npm:pi-cursor-sdk` in `~/.pi/agent/settings.json` and installed under `~/.pi/agent/npm/node_modules/pi-cursor-sdk`.

Use pi's package commands rather than deleting `node_modules` manually:

```bash
pi remove npm:pi-cursor-sdk
pi install /Users/kkk4oru/ghq/github.com/kkkaoru/dotfiles/tools/pi-my-cursor-provider
pi list
pi --list-models cursor
```

Expected `pi list` entry:

```text
./packages/pi-my-cursor-provider
```

Use the `~/.pi/agent/packages/` link created by `create-symlinks.sh`. `~/.pi` is a symlink into this repository, and pi resolves package-relative paths against `~/.pi/agent` without following that symlink. A `../../tools/...` entry therefore points at a nonexistent `~/tools/...` directory instead of this package.

To roll back without changing Cursor authentication or model selection:

```bash
pi remove /Users/kkk4oru/ghq/github.com/kkkaoru/dotfiles/tools/pi-my-cursor-provider
pi install npm:pi-cursor-sdk
```

The provider and model remain `cursor/auto` in either direction.
