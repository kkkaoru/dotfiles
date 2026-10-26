# ~/.dsh — DeepSeek Harness home

`~/.dsh` is a symlink to this directory, installed by `./create-symlinks.sh`.
DeepSeek Harness (`dsh`, the Desktop app, and the bundled CLI) reads
`$DSH_HOME`, which defaults to `~/.dsh`.

## Verify a provider route from the CLI

```sh
ln -sfn "/Applications/DeepSeek Harness.app/Contents/Resources/runtime/cli/bin/dsh" ~/.local/bin/dsh
dsh --profile headless "Reply with exactly: ok"
```

`dsh headless` answers one task and exits, so a printed answer proves the
route, the endpoint, and the credential all work. The headless bundle disables
the on-machine credential store by default; `cordis.patch.yml` re-enables it so
the CLI resolves the same stored keys as the Desktop and Web profiles, and
`profiles/headless/cordis.patch.yml` pins the model headless runs.

## Tracked

- `cordis.patch.yml` — the home-level patch layer: applies to every profile.
  Shared provider routes belong here; the Desktop app never rewrites it.
- `profiles/<name>/cordis.patch.yml` — a profile's user patch layer. The
  Desktop app rewrites this file (its Models and Settings pages write here),
  so expect diffs after UI changes.
- `.env.example` — the template for local secrets.

## Not tracked

`.env`, `.credentials.yaml`, `.anonymous-user-id`, `sessions/`, `storages/`,
and everything under `profiles/<name>/` except the patch layer (the app owns
`cordis.yml`, `package.json`, and `node_modules` there). They stay local so
credentials and machine state cannot leak into git.

Plugins this repository author is kept outside this directory:
[`tools/dsh-dracula-theme`](../tools/dsh-dracula-theme/README.md) is a dsh
bundle installed into a profile with `dsh plugin add`.

## Restore on another Mac

1. Clone this repository and run `./create-symlinks.sh`. An existing `~/.dsh`
   is merged into this directory first (tracked files win), then replaced by
   the symlink.
2. `cp ~/.dsh/.env.example ~/.dsh/.env && chmod 600 ~/.dsh/.env`, then fill in
   the keys referenced by `apiKeyEnv` in `cordis.patch.yml`.
3. Install the theme bundle into the profiles you use, then launch the app or
   `dsh web`:

   ```sh
   dsh plugin --profile web add <checkout>/tools/dsh-dracula-theme
   dsh plugin --profile desktop add <checkout>/tools/dsh-dracula-theme
   ```

   Profile manifests are local (untracked), so this step is what re-creates the
   layer on a new machine. A configured provider credential also skips the
   Desktop welcome/sign-in screen.

Provider credentials stored by the app (`.credentials.yaml`) stay machine
local; `.env` is the portable path and takes effect on the next request.
Either resolves through the `apiKeyEnv` name a route declares, and a stored
value wins over the environment.

## Optional: the `dsh` command

`brew install --cask deepseek-harness` does not put `dsh` on `PATH`. Link the
bundled launcher, for example:

```sh
ln -sfn "/Applications/DeepSeek Harness.app/Contents/Resources/runtime/cli/bin/dsh" ~/.local/bin/dsh
```
