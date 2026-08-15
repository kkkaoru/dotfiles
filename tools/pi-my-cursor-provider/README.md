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

## Development

```bash
bun install
bun run check
pi --approve -e . --model cursor/auto
```

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
