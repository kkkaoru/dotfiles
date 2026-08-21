# pi omlx lifecycle extension

A global [pi extension](https://pi.dev/docs/latest/extensions) that starts and stops the managed
oMLX local server in step with pi's own model selection, rather than only once at process launch.

pi's config-level model catalog (`.pi/agent/models.json`) has no start/stop hook field for a
provider or model, so this cannot be done declaratively in config; it needs pi's extension event
API instead.

- On pi's `model_select` event (fired by `/model`, cycling models, or any other in-session model
  switch — not fired for the initial default model picked at process startup), switching **into**
  the `omlx` provider runs `ensure-omlx` (health-checks and starts the server if needed); switching
  **away** from `omlx` runs `omlx-idle-stop` once as an immediate nudge (it only actually stops the
  server when nothing else is using it — the same idle-stop policy already installed as a
  `launchd` agent by `create-symlinks.sh`, polling every 120 seconds).
- On pi's `session_shutdown` event with `reason: "quit"` (the process is actually exiting), if the
  session's current model is still on `omlx`, the same `omlx-idle-stop` nudge runs. The other
  shutdown reasons (`reload`, `new`, `resume`, `fork`) replace the session in the same still-alive
  process without a matching `model_select` for the replacement session's initial model, so this
  extension ignores them — nudging idle-stop there could stop a server the next session still
  needs with nothing left to restart it.
- Switching between two non-`omlx` providers is a no-op; no extra process is spawned.

This extension never invents its own start/stop logic: it only shells out to the existing
`~/.local/bin/ensure-omlx` (`scripts/ensure-omlx.sh`) and `~/.local/bin/omlx-idle-stop`
(`scripts/omlx-idle-stop.sh`), the same scripts `scripts/pi` already runs once at process launch
for CLI-argument-selected providers. `scripts/pi`'s launch-time check stays in place and stays
load-bearing: since `model_select` never fires for the model a process starts with, this extension
alone would never start omlx for a plain `pi --provider omlx` invocation. The two are complementary
covering different moments — process launch vs. in-session switching — not overlapping/redundant.

Every outcome is caught and turned into a soft `ctx.ui.notify(..., "warning")` instead of an
exception, so a machine with no oMLX install at all (no `~/.omlx`, no `omlx-cli`, unreachable
health endpoint) degrades to a warning toast on model switch — it never breaks the switch itself
or crashes the session.

## Install

From the dotfiles root:

```bash
./create-symlinks.sh
```

This links the extension to `~/.pi/agent/extensions/omlx-lifecycle`. Restart pi or run `/reload`.

## Quality checks

```bash
bun install
bun run check
```

Vitest enforces 95% minimum branch, function, line, and statement coverage. Oxlint enables every
rule category and type-aware checks; Oxfmt is the sole formatter.
