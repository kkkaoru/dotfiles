---
name: executor-and-skills
description: Use for MCP/API integrations, Context7 docs, browser tools, or Cloudflare/Workers/Sandbox, MotherDuck/DuckDB/Dive, GPUI/Rust UI and web-performance tasks. Search and load installed domain skills through Executor before doing the task; discover service tools on demand.
---

# Executor: skills and tools on demand

Use the existing `bash`/`tmux_exec` tools. The `executor` CLI uses Executor Desktop's
shared local catalog and starts its daemon on demand. Do not start a second
`pi-executor` sidecar or load all integration schemas into pi.

## Load domain guidance first

The `local-skills` integration exposes installed Cloudflare, MotherDuck, GPUI,
web-performance and coding-rule skills as **read-only** MCP tools. Their individual
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

OAuth integrations require connection authorization in `executor web` before
service tools exist. Registration alone does not mean they are connected.
Use least-privilege scopes. Cloud operations may modify real resources or incur
costs: skill availability and OAuth login do not authorize those operations.

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
