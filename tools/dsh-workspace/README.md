# dsh-workspace

Register a DeepSeek Harness workspace for a directory, idempotently.

```sh
dsh-workspace                      # the current directory
dsh-workspace ~/ghq/github.com/kkkaoru/my-vrchat
dsh-workspace . --title "My Project"
```

Output is one line on success:

```text
created 8d059cb6-26d3-4927-a3e2-7cfb322069ad /Users/me/ghq/github.com/kkkaoru/dotfiles
exists  8d059cb6-26d3-4927-a3e2-7cfb322069ad /Users/me/ghq/github.com/kkkaoru/dotfiles
```

Exit codes: `0` registered or already registered, `1` the Host refused or the
directory does not exist, `2` bad arguments. `--help` prints usage.

## Why a command and not a dsh subcommand

`dsh <name>` boots a profile, so dsh has no user subcommand surface to extend.
Workspace registration is a Host RPC, so this command drives the same
`workspace/create` call the Desktop window and the Web UI use.

## Idempotency

1. It first reads `$DSH_HOME/storages/workspace.json` and returns `exists`
   without starting anything when the canonical path is already registered.
2. Otherwise it boots a short-lived `dsh web` Host on a free port, authenticates
   with the printed token URL, and calls `workspace/create`. That call returns
   the existing record with `created: false` for a directory that already has a
   workspace, so a race or a stale registry cannot create a duplicate.
3. The path is canonicalized with `realpath`, so `/tmp/x` and `/private/tmp/x`
   resolve to one workspace.

The Host is always stopped, and its log removed, even when the call fails.

## Verification

```sh
cd tools/dsh-workspace
bun install
bun run check          # oxfmt --check, oxlint --type-aware, tsc --noEmit, vitest --coverage
```

`src/cli.ts` is the process boundary (fs, spawn, fetch, timers) and is excluded
from coverage; every decision lives in `src/args.ts`, `src/registry.ts`,
`src/host.ts`, and `src/app.ts`, which are covered above 95%.
