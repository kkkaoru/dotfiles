# pi-my-devin-cli-provider

A pi custom provider that runs the authenticated Devin CLI as a local ACP agent. It does not call Devin's REST API and does not expose pi tools to Devin; `devin acp` executes its own local tools.

## Requirements

- `devin` must be available on `PATH`.
- Authenticate the CLI with `devin auth`.
- Bun and pi 0.84.2 must be installed.

## Install and use

```bash
bun install --cwd tools/pi-my-devin-cli-provider
pi install "$PWD/tools/pi-my-devin-cli-provider"
pi --list-models devin
pi --model devin/swe-1-7 --no-session
```

The dotfiles `create-symlinks.sh` script also links this package under `.pi/agent/packages`, and `.pi/agent/settings.json` loads that relative package path.

## Behavior

- Reuse unit is `(cwd, model, sessionId)`.
- `options.sessionId` from pi (each Agent / subagent) binds a continuing Devin ACP session. When omitted, the provider mints `devin-pi:<uuid>` via `createDevinSessionId()`.
- **First turn / post-compact:** open (or reopen) an ACP session and send the full pi transcript.
- **Later turns:** reuse the same ACP session and send only the latest user text.
- When the transcript contains a pi compaction summary, the ACP session is deleted and the next prompt uses the full compacted transcript on a fresh session.
- `session_before_compact` / `session_compact` also invalidate the pooled Devin runtime for the active pi session so post-compact turns cannot append onto pre-compact Devin history.
- Concurrent subagents A and B with different `sessionId` values keep separate live ACP processes and Devin sessions, even when cwd and model match.
- Idle pooled runtimes exit after two minutes. `pi -p` / `--print` stops the runtime after each request so the CLI can exit promptly.
- Advertised context windows are 80% of Devin's reported limits so pi compacts before the hard limit.
- Compaction summarization uses pi's default path with a fresh `sessionId` (same as stock pi); this package does not redirect summarization to another vendor.
- Sets the selected model with `session/set_config_option` and prefers ACP mode `bypass` when Devin advertises it.
- A fixed non-secret provider marker satisfies pi's custom-provider registration; Devin CLI authentication remains the only authentication used.
- Text and thinking chunks stream back to pi. Devin tool calls stay inside Devin and are not emitted as pi tool calls.
- Permission requests automatically choose `allow_always`, then `allow_once`, then the first available option. The child also receives `DEVIN_PERMISSION_MODE=dangerous`.
- Abort sends `session/cancel` for the active session only; other pooled sessions are left alone.
- Model refresh uses `devin models list --format json`. Static models remain available if pi has not refreshed the catalog.

## Development

```bash
bun install
bun run check
```
