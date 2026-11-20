# pi-graceful-reload

`/reload` refuses while a response or a compaction is running:

```
Warning: Wait for the current response to finish before reloading.
```

`/graceful-reload` does the same reload without the wait: it aborts the running response and the
compaction, waits for them to settle (at most three seconds), and reloads. The partial answer stays
in the transcript, so you can type again immediately.

If a tool ignores the abort and the run is still live after three seconds, pi's own guard wins and
the command reports it in a warning line instead of reloading.

Only public extension API is used, so an upgrade cannot break it; the startup patch in
`scripts/pi-compaction-singleflight.mjs` makes the built-in `/reload` behave the same way.

## Install

`./create-symlinks.sh` links this directory to `~/.pi/agent/extensions/graceful-reload`, which pi
loads automatically. Start a new session after changing it.

## Check

```sh
bun run check
```
