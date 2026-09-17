# Repository guide

This repository manages a personal macOS development environment and local coding-agent
integrations. These instructions apply to agents working here, including pi and Claude Code.

## Commands

- `./create-symlinks.sh` — Link repository-managed configuration into `$HOME`.
  Review changes before running: this updates the live development environment.
- `bun install` / `bun add <package>` / `bunx <package>` — Use Bun for package management.
- `./scripts/setup-executor.sh` — Register shared Executor integrations.
  `--approve-registration` explicitly permits registration and connection creation only.
- `./scripts/executor-cloudflare-auth.sh <cloudflare-integration>` — Start the service's
  OAuth setup and open the authorization page. Login and consent are the user's actions.
- `./scripts/setup-omlx.sh` — Download Qwen3.8-27B-4bit and DFlash2, then restart oMLX.
- `./init-ssh.sh` — Initialize SSH configuration.
- `./scripts/cleanup-deepswe-disk.sh` — Remove legacy DeepSWE/Pier images and job directories;
  run only when cleanup is requested.

## Configuration layout

- `.agents/` — Entire shared Agent Skills home (`~/.agents` is a symlink).
  `skills/` holds generic guidance; `skills-stroage/` holds optional domain skills.
  Use the global `project-skills` skill to copy selected skills into a project's
  `.agents/skills/`. `.skill-lock.json` retains upstream provenance. agmsg DB,
  runtime, and local teams are ignored.
- `.config/fish/` — Fish shell configuration. `config.fish` sources `aliases.fish`,
  `envs.fish`, `binds.fish`, and `path.fish`; Homebrew and mise are integrated here.
- `.config/mise/`, `.config/anyenv/` — Version-management configuration.
- `.gitconfig`, `.gitignore_global`, `.tmux.conf`, `.tmux.session.conf`, `.vimrc`,
  `.vscode/settings.json` — Development-tool configuration.
- `.pi/agent/settings.json` — pi model/provider defaults, package loading and skill exclusions.
  Read the current file rather than assuming a fixed model or version.
- `tools/pi-*/` — Local pi extensions and providers, including agmsg delivery, persistent goals,
  loops, detached tmux execution, effort management, and oMLX lifecycle management.
- `tools/claudex-*/` — Claudex adapters and routing/tool policies.
- `tools/executor-skills/` — Read-only skills MCP, agmsg MCP adapter, and pi routing package.
- `.omlx/` — oMLX configuration; `~/.omlx` is a repository symlink.

`~/.pi` is also linked into this repository. Changing these files can affect running agents.
Preserve unrelated working-tree changes and inspect each component's README/package scripts
before modifying it.

## pi, skills and Executor

Use Executor Desktop's bundled CLI through `scripts/executor` (also available as `executor`).
Do not install a duplicate Executor runtime or `pi-executor` sidecar.

- The generic skill set retains Python/Rust/Swift/TypeScript coding rules,
  git-commit-by-feature, find-skills, project-skills and agmsg/history guidance.
  The `executor-and-skills` router remains package-loaded. Follow the relevant language skill.
- Select optional domain guidance from `.agents/skills-stroage/` using `project-skills`.
  Only selected copies belong in a target project's `.agents/skills/`. Executor's
  `local-skills` setup includes storage for on-demand read-only discovery; existing
  registrations require an explicit setup update. agmsg/history remain routed on demand.
  Do not globally exclude domain names: that would hide selected project copies too.
- Skill text is guidance, not executable functionality or authorization. Discover the actual
  service tool, describe its schema, then call it through Executor. Keep results bounded.
- `.mcp.json` currently defines Context7 and Chrome DevTools stdio servers. Cloudflare and
  MotherDuck remote registrations are defined in `tools/executor-skills/integrations.json`.
  The setup script also registers `local-skills`, `agmsg`, and the official `ctx mcp serve`.
- Model-initiated agmsg operations and ctx searches go through Executor. The pi router carries
  the selected agmsg identity and originating project from pi's session state. Verify the
  identity rather than choosing another registered name. The original agmsg extension still
  provides automatic incoming delivery and user-facing `/agmsg` commands; do not double-poll.
- ctx searches use the existing local index. Supply explicit workspace/session filters and
  inspect source events before relying on prior history. Report missing index coverage.
- Registration is not OAuth authentication. Check actual tool results; CLI exit zero can mean
  a paused approval or a tool-level error. Do not change approval policies without permission.

See `tools/executor-skills/SETUP.ja.md` and the router skill for detailed commands and safeguards.
Run `/reload` or start a new pi session after changing loaded resources. A new session also
avoids retaining old tool/skill descriptions in conversation history.

## Long-running work and verification

Use `tmux_exec` for potentially blocking or duration-uncertain work. Set a realistic
`estimatedDurationSeconds`; an overdue check-in is not completion. For open-ended streams such
as `wrangler tail`, also supply `timeoutSeconds`. Hard deadlines require GNU coreutils on
macOS. Inspect output and exit status when notified; do not leave duplicate watchers running.
For an active self-paced `/loop`, continue actionable work, schedule a later check, or explicitly
finish only when complete or blocked on user input.

`/goal <objective>` explicitly starts a persistent goal; `/goal pause`, `resume`, and `clear`
control its future continuations. Agents may also deliberately use `start_goal` and `start_loop`
for work grounded in the user's established request, without waiting for slash commands. Reuse
existing goals/loops, never invent unrelated tasks or permissions, and never bypass a manual pause
or safe-mode stop by creating another task. Only the user can resume stopped automation. Goals defer to existing loop pacing and tmux notifications.
Pause/clear do not abort the current turn, stop independent loops, or kill detached jobs. Optional
`--tokens N` budgets are soft per-agent-run limits, not billing caps or external-agent budgets.
See `tools/pi-goal-extension/README.md` for completion audits, stall safeguards and restoration.

Verification is component-specific; the root package does not provide a universal test suite:

- `tools/executor-skills/`: `bun run tsc && bun run lint && bun run test`
- `tools/pi-goal-extension/`: `bun run check`; `bun run smoke` for offline native SDK/tmux integration
- `tools/pi-loop-extension/`, `tools/pi-tmux-timeout-extension/`: `bun run check`
- Other components: inspect their `package.json` and README for required checks.
- Shell changes: check syntax; for all changes, run `git diff --check`.

## Secrets

Keep credentials out of Git, prompts and command output. Executor owns integration credentials
and OAuth tokens. Configure other services only as needed; `.env.example` contains optional
and legacy entries, not requirements for every task. Do not print the live `.env`.
