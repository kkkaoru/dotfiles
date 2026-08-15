# pi-my-cursor-provider

Minimal Cursor agent bridge for pi and Claudex. It preserves the existing `cursor/auto` model identity while using Cursor's agent SDK rather than pretending Cursor exposes a raw inference API.

## Guarantees

- Creates a fresh local Cursor agent for each independent request; it never calls `Agent.resume()`.
- Keeps a live Cursor run only across the tool-result continuation belonging to that same request.
- Converts pi `Context.tools` schemas into Cursor SDK `customTools`.
- Returns Cursor custom-tool callbacks as normal pi tool calls and resolves them from the next `ToolResultMessage` context.
- Leaves Cursor-native tools enabled.
- Discovers the authenticated Cursor model catalog and provides a multi-model fallback catalog when discovery is unavailable.
- Uses pi's supplied system prompt and tools without reloading ambient Cursor setting sources, avoiding duplicate rules and SDK bootstrap logs in standalone TUI use.

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

| Input                                             | Behavior                                                                                                                                                                                                                                       |
| ------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Model ID, Cursor credential, abort signal         | Passed to the Cursor agent.                                                                                                                                                                                                                    |
| Pi tools                                          | Converted to Cursor custom tools with names, descriptions, and JSON schemas; results return to the same live run.                                                                                                                              |
| System prompt and message history                 | Transformed into one role-labelled transcript because each independent request uses a fresh Cursor agent.                                                                                                                                      |
| Images                                            | Every user and tool-result image in the history is attached again in transcript order. Numbered placeholders such as `[image 2 attached: image/jpeg]` associate transcript positions with the SDK image array. Duplicate images are preserved. |
| Reasoning level                                   | Deferred: Pi receives the requested level, but this provider does not yet map it to model-specific Cursor SDK parameters. Cursor Auto exposes no documented effort parameter.                                                                  |
| Maximum output tokens and temperature             | Not configurable through the Cursor Agent SDK. Pi model values are descriptive metadata only.                                                                                                                                                  |
| Session ID, cache retention, and request metadata | Not configurable through the Cursor Agent SDK. Independent requests intentionally create fresh agents.                                                                                                                                         |

The SDK publishes no image count or byte limit, so the provider never silently truncates image history. An upstream limit is returned as an explicit request error. Reattaching all earlier images on every independent turn preserves correctness but increases payload size and latency in image-heavy conversations.

## Development

```bash
bun install
bun run check
pi --approve -e . --model cursor/auto
```

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
../../ghq/github.com/kkkaoru/dotfiles/tools/pi-my-cursor-provider
```

To roll back without changing Cursor authentication or model selection:

```bash
pi remove /Users/kkk4oru/ghq/github.com/kkkaoru/dotfiles/tools/pi-my-cursor-provider
pi install npm:pi-cursor-sdk
```

The provider and model remain `cursor/auto` in either direction.
