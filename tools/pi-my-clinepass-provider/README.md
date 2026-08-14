# pi my ClinePass provider

A private pi provider that uses the installed `cline` CLI as the source of truth
for the current ClinePass model list. It preserves ClinePass WorkOS/static-key
authentication and uses pi's built-in OpenAI Chat Completions transport.

This implementation is based on the MIT-licensed
[`jellydn/pi-clinepass-provider`](https://github.com/jellydn/pi-clinepass-provider)
and follows the development layout and quality settings of the local
`tools/pi-agmsg-extension` package.

## Why this exists

The upstream provider's published fallback catalog lagged behind Cline CLI and
its `/api/v1/models` discovery endpoint returned 404 even with a valid WorkOS
token. As a result, models such as `cline-pass/glm-5.3` were visible in Cline
but absent from pi.

This provider starts `cline --acp`, initializes the official Agent Client
Protocol connection, creates an in-memory ClinePass session, and reads
`models.availableModels` from the response. Only `cline-pass/*` IDs are
registered. Pi model refreshes repeat the same discovery, so updating Cline also
updates pi's selectable catalog without changing this extension.

The CLI supplies model identity, name, description, and availability. Local
metadata supplies pi-specific context/output limits, pricing references,
reasoning maps, and compatibility flags. Unknown future CLI models receive
conservative metadata until reviewed, but are still selectable.

## Requirements

- Node.js 22+
- Bun
- pi 0.84.2+
- `cline` available on `PATH`
- Cline authenticated with `cline auth`

## Install

From the dotfiles repository root:

```bash
bun install --cwd tools/pi-my-clinepass-provider
pi install "$PWD/tools/pi-my-clinepass-provider"
```

Restart pi or run `/reload` after installation.

## Authentication

The provider ID remains `clinepass`, so existing credentials in
`~/.pi/agent/auth.json` continue to work. `/login` can also import WorkOS
credentials from `~/.cline/data/settings/providers.json` or accept a static
ClinePass API key. WorkOS refreshes use Cline's token refresh endpoint.

## Verification

```bash
cline --version
pi --list-models clinepass
bun run --cwd tools/pi-my-clinepass-provider check
```

The model list should include every `cline-pass/*` model currently returned by
the installed Cline CLI, including `cline-pass/glm-5.3` when supported by that
CLI version.
