# pi-claude-agent-sdk-provider

A pi custom provider (`claude-sdk`) that runs Claude through the official
[Claude Agent SDK](https://code.claude.com/docs/en/agent-sdk/typescript) with the local Claude
Code login, so requests draw from the Claude plan's usage limits instead of API billing or
extra usage. Pi keeps its own agent loop: the model sees pi's tools and pi executes every call.

## Requirements

- A Claude Code login for a Claude plan (`claude auth status` shows `authMethod: "claude.ai"`).
- Bun and pi 1.0.4+. The SDK installs its own native Claude Code binary.

## Install and use

```bash
bun install --cwd tools/pi-claude-agent-sdk-provider
pi --list-models claude-sdk
pi --model claude-sdk/claude-haiku-5-5
```

The dotfiles `create-symlinks.sh` script links this package under `.pi/agent/packages`, and
`.pi/agent/settings.json` loads that relative package path.

## How it works

- Each pi session (`options.sessionId`) owns one long-lived SDK `query()` in streaming-input mode.
- Claude Code's built-in tools are disabled (`tools: []`). Pi's current tools are exposed as an
  in-process MCP server `pi` using their JSON schemas, with `anthropic/alwaysLoad` so they are
  never deferred. `settingSources: []` and `strictMcpConfig` keep user settings, hooks, plugins,
  CLAUDE.md and MCP config out of the child. Pi's system prompt replaces Claude Code's.
- A pi turn ends at the API `message_stop` with `stop_reason: tool_use`. Pi runs the tools; the
  next turn settles the waiting MCP calls by `tool_use` id (`_meta["claudecode/toolUseId"]`),
  and the SDK continues the same conversation. A `result` message ends a turn with `stop`.
- The session continues only when pi's transcript extends it exactly: results for every awaited
  call, or new user input after a finished reply. Compaction, branching, steering mixed into
  tool results, or changed system prompt, tools, model, cwd or thinking on/off start a fresh
  session that replays the transcript as text. Effort changes apply live via `applyFlagSettings`.
- `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN` and `ANTHROPIC_BASE_URL` are removed from the child
  environment so the plan login is always used, even inside a claudex shell.
- Sessions close after 15 idle minutes, on abort, on error, and after each reply in `pi -p`.
- Cost is reported as zero; token counts come from the API usage in the stream.

## Limits

- `src/models.ts` lists the canonical ids that the SDK's `supportedModels()` returned for a Pro
  login (2026-10-19). Haiku 4.5 has no adaptive thinking, so it is listed without reasoning.
- When a plan window is exhausted the API says "out of extra usage"; errors append the last
  `rate_limit_event` window, status and reset time.
- A restart replays history as text, so earlier images and exact tool-call structure are lost.
  Mid-conversation system-message updates (from other pi extensions) trigger such restarts.
- Usage counts against the same plan limits as Claude Code itself.

## Development

```bash
bun install
bun run check
```
