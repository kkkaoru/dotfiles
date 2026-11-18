# Repository guide

This repository manages a personal macOS development environment and local coding-agent
integrations. These instructions apply to agents working here, including pi and Claude Code.

## Commands

- `./create-symlinks.sh` — Link repository-managed configuration into `$HOME`.
  Review changes before running: this updates the live development environment.
- `bun install` / `bun add <package>` / `bunx <package>` — Use Bun for package management.
- `./scripts/secrets.sh keygen|encrypt|decrypt|identity <name>` — Keep a secret in Git only as
  `secrets/<name>.age` (age). The age identity lives in 1Password (`AGE_OP_REF`), else the macOS
  keychain (`AGE_KEYCHAIN_SERVICE`), else `~/.config/age/keys.txt`, so every Mac of the owner's
  decrypts it without copying a file; `secrets/recipients.txt` lists their public keys. Re-encrypt
  after adding a key so every Mac can read every file.
- `./scripts/setup-executor.sh` — Register shared Executor integrations.
  `--approve-registration` explicitly permits registration and connection creation only.
- `./scripts/executor-cloudflare-auth.sh <cloudflare-integration>` — Start the service's
  OAuth setup and open the authorization page. Login and consent are the user's actions.
- `./scripts/setup-omlx.sh` — Download Qwen3.8-27B-4bit and DFlash2, then restart oMLX.
- `./scripts/setup-plamo-translate.sh` — Set up PLaMo 3 Translate 0.8B for `tools/claude-plamo-translate`
  and `tools/pi-plamo-translate`
  (run once per machine; the model is not in Git): mlx-lm + MLX 4-bit on macOS, llama.cpp's Vulkan
  build + the official GGUF on Linux and Windows (Git Bash).
- `./scripts/setup-herdr-app.sh` — After `./create-symlinks.sh`, set up Herdr.app: build the font
  `settings.json` names (if missing) and switch the open window to the `main-emoji` layout.
- `./scripts/build-udev-gothic-light.sh [Light|ExtraLight|Thin]` — Build UDEV Gothic <weight> 35NFLG
  (JetBrains Mono Latin glyphs at that weight; installs fontforge) into `~/Library/Fonts` for Herdr.app's
  terminal font, which uses Thin.
- `./scripts/strata-tunnel.sh` — Forward Strata's OpenAI-compatible API from the GPU host
  (SSH alias `my-win`) to `127.0.0.1:18080`, where the `strata-win` provider in
  `.pi/agent/models.json` expects it. The `strata` provider is the Strata running on this Mac
  at `127.0.0.1:8080`. Change the remote target with `STRATA_SSH_HOST`, `STRATA_REMOTE_ADDR`,
  `STRATA_REMOTE_PORT` or `STRATA_LOCAL_PORT`; the remote API key stays in `~/.pi/agent/auth.json`.
- `./scripts/strata-local.sh` — Start this Mac's Strata as a managed instance on `127.0.0.1:8080`
  for the `strata` provider (profile `launch-q2_0-32k`, ~30 s warm-up). Nothing starts it in
  advance: `tools/pi-strata-lifecycle` runs this when a `strata` model is selected, and
  `./scripts/strata-idle-stop.sh` (launchd `com.kkkaoru.strata-idle-stop`, every 2 min) stops it
  after `STRATA_IDLE_MINUTES` (default 10) with no request and no client connection.
  `./scripts/strata-stop.sh` drains it on demand; all take `STRATA_PROFILE`/`STRATA_LOCAL_PORT`.
- `./scripts/ensure-strata-win.sh` — Start the GPU host's Strata on demand: runs its `StrataStart`
  scheduled task (no trigger — only `schtasks /run` starts it) and brings the tunnel up. The same
  `tools/pi-strata-lifecycle` extension calls it when a `strata-win` model is selected and before
  each provider request, so a server the idle timer stopped mid-session comes back by itself;
  my-win's `StrataIdleStop` task (`scripts/strata-win-idle-stop.ps1`, every 5 min) stops it after
  10 idle minutes. Servers therefore exist only while a Strata model is in use.
- `./scripts/setup-bonsai.sh` — Install Ternary Bonsai 2 27B (MLX 2-bit) and symlink it into
  `~/.omlx/models` so pi's `bonsai` provider is that oMLX model, not a second server. `--link <dir>`
  moves an existing download to `~/Models/Ternary-Bonsai-2-27B-mlx-2bit` and leaves a symlink.
- `./scripts/ensure-bonsai-mac.sh` — Start the local oMLX when
  https://bonsai-mac.kkk4oru.com is not listing the Bonsai model. That hostname is on the
  `ssh-my-mac` tunnel (`./scripts/cf-expose-http.sh bonsai-mac 8891 ssh-my-mac`), aimed at oMLX
  port 8891 on this Mac. pi's `tools/pi-strata-lifecycle` calls it before a `bonsai-mac` request, and
  the `bonsai-mac` provider in `.pi/agent/models.json` reaches the tunnel with the `pi-strata`
  service token. Local `bonsai` starts oMLX only when that provider is selected
  (`tools/pi-omlx-lifecycle-extension`); the model is unpinned and unloads after 10 idle minutes.
  `bonsai-serve.sh` is the retired mlx_vlm server and is not used by pi.
- `./scripts/cf-expose-http.sh <label> <port> [tunnel]` — Add `https://<label>.kkk4oru.com` to an
  existing tunnel's ingress, with an Access application that only the `pi-strata` service token
  and the owner's email may pass. Use it instead of opening a port to serve a new model endpoint.
- `./scripts/strata-awake.sh` — Print one line per remote endpoint (`strata-win`, `strata-mac`,
  `bonsai-mac`, `strata-colab`): `awake`, `asleep` (Cloudflare 502 or no answer) or `unknown`
  (no colab URL yet). Read-only; it never starts a server. `tools/pi-strata-lifecycle` runs it at
  session start and every minute to keep that state in the pi footer, and warns when an endpoint
  the idle timer is about to stop is still in use.
- `./init-ssh.sh` — Initialize SSH configuration.
- `./scripts/cf-remote-access.sh <label> [--agent]` — Publish one machine's SSH server as
  `<label>.kkk4oru.com` through a remotely managed Cloudflare Tunnel plus an Access application
  that allows only the logged-in Cloudflare user. Creates the tunnel, ingress, Access app/policy
  and proxied CNAME idempotently, writes the connector token to `~/.cloudflared/ssh-<label>.token`,
  and prints the `cloudflared access ssh` / `~/.ssh/config` lines for the other machines.
  `--agent` also installs and loads a launchd agent on macOS. Hostnames stay first-level because
  Universal SSL does not cover `*.<label>.<zone>`.
- `./scripts/cleanup-deepswe-disk.sh` — Remove legacy DeepSWE/Pier images and job directories;
  run only when cleanup is requested.

## Configuration layout

- `.agents/` — Entire shared Agent Skills home (`~/.agents` is a symlink).
  `skills/` holds generic guidance; `skills-stroage/` holds optional domain skills.
  Use the global `project-skills` skill to hard-link selected skills into a project's
  `.agents/skills/`. `.skill-lock.json` retains upstream provenance.
- `.config/fish/` — Fish shell configuration. `config.fish` sources `aliases.fish`,
  `envs.fish`, `binds.fish`, and `path.fish`; Homebrew and mise are integrated here.
- `.config/mise/`, `.config/anyenv/` — Version-management configuration.
- `.gitconfig`, `.gitignore_global`, `.tmux.conf`, `.tmux.session.conf`, `.vimrc`,
  `.vscode/settings.json` — Development-tool configuration.
- `.pi/agent/settings.json` — pi model/provider defaults, package loading and skill exclusions.
  Read the current file rather than assuming a fixed model or version.
- `tools/pi-*/` — Local pi extensions and providers, including persistent goals,
  loops, detached tmux execution, effort management, oMLX lifecycle management, trimming requests
  to a model's declared image caps (`pi-image-budget`), the AI Gateway's own daily cost in the
  footer (`pi-gateway-cost`), and the `x-session-affinity` header that keeps a session's Workers AI
  prompt prefix cached (`pi-cf-affinity`). They are linked by `create-symlinks.sh`.
- `tools/pi-my-antigravity/` — Subscription-backed Antigravity (Cloud Code Assist) provider that
  mirrors the official `agy` CLI wire format (`bun run check`). Sends the system prompt as the
  leading user turn because the backend 429s third-party prompts in `systemInstruction`.
- `tools/claudex-*/` — Claudex adapters and routing/tool policies.
- `.claude-plugin/marketplace.json` — The `dotfiles` plugin marketplace that lists the `tools/claude-*`
  mods. `./create-symlinks.sh` registers the checkout where it is cloned (in the ignored
  `.claude/settings.local.json`); user settings enable each mod as `<name>@dotfiles`, read in place.
- `tools/claude-dynamic-effort/` — Deprecated: kept as code but disabled in user settings. Claude Code
  mod that runs every compaction at xhigh effort and every other request at the effort you chose
  (`claude plugin validate|test <dir>`).
- `tools/claude-goal-pacing/` — Claude Code mod that ends an idle `/goal` continuation instead
  of re-checking it at once (`bun run check`).
- `tools/claude-plamo-translate/` — Claude Code mod that translates prompts (default en) and the
  text Claude shows you (default ja) with PLaMo 3 Translate on a local `mlx_lm.server` or llama.cpp
  `llama-server` (`bun run check`).
- `tools/pi-plamo-translate/` — pi package with the same server: you see Japanese, typed Japanese
  becomes English, and the agent reads and writes English (`bun run check`).
- `tools/herdr-app-mods/` — Tests, oxlint and oxfmt for the Herdr.app mods in
  `.config/herdr-app/mods/` (host modules mocked); every mod file keeps >=95% coverage.
- `tools/dsh-dracula-theme/` — A DeepSeek Harness bundle (no build step) that
  injects the Dracula palette into the Web and Desktop clients. Install it into
  a profile with `dsh plugin --profile <name> add <checkout>/tools/dsh-dracula-theme`.
- `tools/dsh-workspace/` — TypeScript CLI (`bun`, `bun run check`) that registers a
  directory idempotently through the Harness `workspace/create` RPC. Linked to
  `~/.local/bin/dsh-workspace`.
- `tools/executor-skills/` — Read-only skills MCP and its pi guidance package.
- `.omlx/` — oMLX configuration; `~/.omlx` is a repository symlink.

`~/.pi` is also linked into this repository. Changing these files can affect running agents.
Preserve unrelated working-tree changes and inspect each component's README/package scripts
before modifying it.

## pi, skills and Executor

Use Executor Desktop's bundled CLI through `scripts/executor` (also available as `executor`).
Do not install a duplicate Executor runtime or `pi-executor` sidecar.

- The generic skill set retains C#/Python/Rust/Swift/TypeScript coding rules,
  git-commit-by-feature, find-skills, project-skills and history guidance.
  The `executor-and-skills` router remains package-loaded. Follow the relevant language skill.
- Select optional domain guidance from `.agents/skills-stroage/` using `project-skills`.
  Selected skills are hard-linked into a target project's `.agents/skills/`; use
  `manage.sh status/update/remove` to inspect or change an installation. Executor's
  `local-skills` setup includes storage for on-demand read-only discovery; existing
  registrations require an explicit setup update. History remains routed on demand.
  Do not globally exclude domain names: that would hide selected project copies too.
- Skill text is guidance, not executable functionality or authorization. Discover the actual
  service tool, describe its schema, then call it through Executor. In pi, use the native MCP
  `executor` server via codemode; the CLI remains available for administration and fallback.
  Keep results bounded.
- `.pi/agent/mcp.json` connects pi's native MCP client to `executor mcp` with browser approvals.
  Executor retains its catalog, credentials and policies; no duplicate registrations or OAuth
  token copies are needed. `.mcp.json` remains the other clients' Context7/Chrome DevTools config.
  Cloudflare and MotherDuck registrations are defined in `tools/executor-skills/integrations.json`.
  The setup script also registers `local-skills` and the official `ctx mcp serve`.
- Model-initiated ctx searches go through Executor. agmsg was removed from this repository and
  from its Executor registration: no agmsg command, hook, adapter or copy remains.
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
finish only when complete or blocked on user input. Loop state and waiting tasks use pi-durable;
keep each session's `.loop-durable/` sibling directory with its session file. It resumes when
that Pi session is reopened, not while Pi is closed. See `tools/pi-loop-extension/README.md`.
Tmux bookkeeping/reconciliation also uses pi-durable (`.tmux-durable/` beside the session);
tmux remains the detached OS executor. Keep the sidecar with the session file. Recovery never
re-executes commands. See `tools/pi-tmux-timeout-extension/README.md`.

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
- `tools/claude-dynamic-effort/`: `bun run check` (vitest coverage and `claude plugin validate`)
- `tools/claude-plamo-translate/`: `bun run check`
- `tools/pi-plamo-translate/`: `bun run check`
- `tools/pi-image-budget-extension/`: `bun run check`
- `tools/pi-gateway-cost-extension/`: `bun run check`
- `tools/pi-cf-affinity-extension/`: `bun run check`
- `.config/herdr-app/mods/`: `bun run check` in `tools/herdr-app-mods/` (lint, format, per-file 95% coverage)
- Other components: inspect their `package.json` and README for required checks.
- Shell changes: check syntax; for all changes, run `git diff --check`.

## Secrets

Keep credentials out of Git, prompts and command output. A value that settings or a tool needs
goes into `secrets/<name>.age` with `./scripts/secrets.sh`; never commit its plaintext. Executor owns
integration credentials and OAuth tokens. Configure other services only as needed; `.env.example` contains optional
and legacy entries, not requirements for every task. Do not print the live `.env`.
