---
name: executor-and-skills
description: Use for cross-agent messaging (agmsg), prior agent history (ctx), MCP/API integrations, Apple Pro apps (Motion, Compressor, Final Cut Pro, Logic Pro, MainStage), browser tools, Cloudflare, MotherDuck, GPUI and web performance. Load guidance and discover execution tools through Executor on demand.
---

# Executor: skills and tools on demand

Use the existing `bash`/`tmux_exec` tools. The `executor` CLI uses Executor Desktop's
shared local catalog and starts its daemon on demand. Do not start a second
`pi-executor` sidecar or load all integration schemas into pi.

## Load domain guidance first

The `local-skills` integration exposes installed agmsg, ctx-agent-history-search,
Cloudflare, MotherDuck, GPUI, web-performance and coding-rule skills as **read-only** MCP tools. Their individual
descriptions are excluded from pi's startup prompt, not deleted from disk.

1. Discover with `executor tools search 'search_skills' --namespace local-skills --limit 3`.
2. Describe the exact returned path with `executor tools describe '<path>'`.
3. Call that path with a focused `query` and `limit` (1–20). Search terms are
   ANDed; prefer `cloudflare`, `motherduck`, `gpui`, or `sandbox` rather than a sentence.
4. Discover/call `read_skill` for the selected ID. Read the full instructions
   **before** planning or implementing the domain task.
5. Follow references with `read_reference` using a relative `path`, `offset: 0`
   and `limit: 24000`. Continue with `nextOffset` until null when the full file is needed.
   Relative paths resolve against the selected skill's canonical directory.

If the integration is unavailable, use this local fallback, then `read` the
matching SKILL.md and its references; never claim Executor was used:

```bash
rg --hidden --follow -n -g SKILL.md '^(name|description):' \
  "$HOME/.agents/skills" "$HOME/.pi/agent/skills"
```

Trusted project `.agents/skills` and `.pi/skills` remain local to that project.
The shared Skills MCP includes the dotfiles coding rules, not arbitrary projects.
Skills are instructions, not authorization to run their commands or alter data.

## Discover and call service tools

```bash
executor tools integrations
executor tools search 'documentation' --namespace cloudflare-docs --limit 5
executor tools search 'read_skill' --namespace local-skills --limit 3
executor tools describe '<exact.path.from.search>'
executor call <exact path segments from discovery> '{"argument":"value"}'
```

Search, describe only the chosen tool, then call its exact schema-valid path.
Do not guess namespaces or print every schema. Keep outputs bounded. Use
`tmux_exec` for network calls and uncertain-duration work.

| Task | Integration |
|---|---|
| Cross-agent identity, messages, team and history | `agmsg` |
| Prior coding-agent sessions and decisions | `ctx` |
| Cloudflare docs / Agents SDK docs | `cloudflare-docs` / `cloudflare-agents` |
| DNS, Workers, R2, D1, KV, Zero Trust and other Cloudflare API operations | `cloudflare-api` (nested Code Mode: discover its search/execute schema first) |
| Storage/AI/compute bindings | `cloudflare-bindings` |
| Workers build/deploy diagnostics | `cloudflare-builds` |
| Workers logs/metrics | `cloudflare-observability` |
| Remote browser, screenshots, Markdown | `cloudflare-browser` |
| Cloudflare analytics | `cloudflare-graphql` |
| SQL, database exploration, Dive | `motherduck` |
| Library documentation | `context7` |
| Local Chrome / performance | `chrome-devtools` |
| Apple Pro apps: native Swift CLI/XML/MIDI/OSC integration | `apple-pro-apps` |
| Apple Pro apps: UI-only fallback using existing Peekaboo | `apple-pro-apps-ui` |

For Apple Pro apps, load `apple-pro-apps` and the matching app skill first. Prefer
native machine interfaces; discover `app_capabilities` for actual installed editions
and limitations. Only use the separate UI namespace when no suitable native method
exists. MIDI/OSC routing, app licenses and macOS permissions are not granted by setup.

OAuth integrations require connection authorization before service tools exist.
Registration alone does not mean they are connected. For Cloudflare, use the
`executor-cloudflare-auth.sh` helper from the dotfiles `scripts/` directory: it
runs discovery, dynamic client registration, and OAuth start through Executor,
then opens the returned authorization URL directly rather than relying on the
management UI popup. Read the local setup guide at `../../SETUP.ja.md` with
`read` (outside this skill directory, so not `read_reference`) for commands and
approval flags. The user must complete login and consent in their browser.
Use `executor web` for connection status and other providers such as MotherDuck.
Use least-privilege scopes. Cloud operations may modify real resources or incur
costs: skill availability and OAuth login do not authorize those operations.

## Agent messaging and history

Use Executor for **model-initiated** agmsg and ctx operations, not direct shell scripts or
Pi's old `agmsg` tool. First read the corresponding skill through `local-skills`.
The router extension disables only the direct model tool; Pi's existing agmsg extension
still owns automatic incoming delivery and the user's `/agmsg` setup commands.

- For messages, discover tools with `executor tools search 'agmsg_whoami' --namespace agmsg --limit 2`.
  Pass the **originating Pi project's absolute path**, not Executor's cwd. The router supplies
  the active identity from Pi's own session entry when available; match that name and team
  against `agmsg_whoami`. If multiple identities exist and the active name is unknown,
  ask the user to confirm with `/agmsg whoami`. Never silently pick another session's sender.
- `agmsg_send`, `agmsg_history`, and `agmsg_inbox` require explicit `project`, `team`, and `agent`.
  The server verifies the pair is registered for that project with agent type `pi` before
  using the official scripts. `agmsg_inbox` marks messages read: use it only for an explicit
  one-time inbox request, never periodic polling. Do not blindly retry a send.
- The installed agmsg skill has Codex-specific examples. In Pi, retain agent type `pi` and
  existing Pi delivery. Do not install Codex monitor hooks, change identities, or spawn agents
  as part of ordinary message operations. Setup/identity changes remain user-driven `/agmsg`
  commands; the Executor integration deliberately does not expose them.
- Proactively use `ctx` when earlier sessions may contain relevant decisions or failed attempts.
  Discover the native ctx MCP tools on demand. Bound searches and supply the originating
  workspace/session filters explicitly: a shared MCP process cannot infer Pi's active session.
  Native `ctx.search` reads the existing index without imports or refresh; its schemas use
  snake_case (for example `primary_only`). It does not currently expose `exclude_session`:
  do not claim automatic current-session exclusion; inspect/filter matching session hits.
  Inspect cited events before relying on them. Keep transcripts private; no automatic imports,
  reindexing, exports, or telemetry changes as part of this migration.
- These services stay local to Executor Desktop. If unavailable, report the failure rather
  than silently bypassing Executor or reading agmsg storage directly. No new permanent
  approval policy is implied by enabling them.

## Management and approvals

Manage integrations, connections, credentials and policies with `executor web`.
Inspect management schemas with `executor call executor --help` and `tools describe`.
Use credential handoff UI, never repository secrets or token-bearing shell arguments.
Do not weaken approval policies.

If execution pauses, show the user the exact action/arguments and approval URL.
Resume only with explicit approval covering that action and the exact execution ID:

```bash
executor resume --execution-id '<id>' --action accept --content '{}'
```

Use content matching the requested schema; `{}` only fits empty schemas. Never
blindly approve nested prompts. A zero CLI exit can still mean `Execution paused`
or a tool error. Verify the result and connection health before reporting success.
