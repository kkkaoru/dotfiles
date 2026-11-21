# pi-graceful-reload

`/reload` refuses while a response or a compaction is running:

```
Warning: Wait for the current response to finish before reloading.
```

`/graceful-reload` does the same reload without the wait: it aborts the running response and the
compaction, waits for them to settle (at most three seconds), then reverts the interrupted turn —
the session returns to where it stood when that run started, and the prompt that started it goes
back into the editor, so the work can run again instead of stopping half-finished. Finally it
reloads.

pi reports the leaf from _before_ the prompt (`agent_start` fires ahead of the prompt being
persisted), so the revert navigates to the interrupted prompt itself: pi moves the leaf back to the
answer before it and returns that prompt's text for the editor. Navigating to the recorded leaf
instead would drop the prompt along with the partial answer, which is how an interrupted turn used
to disappear. A message an extension injected is dropped the same way, without taking the editor.
If the branch has moved away from where the run started, nothing is reverted: the reload still
happens, but no other work is dropped.

If a tool ignores the abort and the run is still live after three seconds, pi's own guard wins: the
revert is skipped, and the command reports the refusal in a warning line instead of reloading. An
editor that already holds text keeps it.

The revert lives in memory, as pi's own tree navigation does: it survives the reload, and the next
message makes it permanent, but quitting pi before typing anything resumes at the aborted turn.

Only public extension API is used, so an upgrade cannot break it; the startup patch in
`scripts/pi-compaction-singleflight.mjs` makes the built-in `/reload` behave the same way.

## Install

`./create-symlinks.sh` links this directory to `~/.pi/agent/extensions/graceful-reload`, which pi
loads automatically. Start a new session after changing it.

## Check

```sh
bun run check
```
