# pi tmux timeout extension

A global [pi extension](https://pi.dev/docs/latest/extensions) that keeps pi responsive while
long-running commands continue in detached tmux sessions.

## Behavior

- Registers the parallel `tmux_exec` tool for explicitly starting a command in detached tmux.
- Automatically rewrites `bash` tool calls with a timeout of at least 120 seconds.
- Automatically rewrites known blocking watchers: `gh run watch`, `watch`, and `tail -f`.
- Leaves short commands and commands that already invoke tmux unchanged.
- Stores combined stdout/stderr in `output.log` and the numeric exit code in `exit-status` under the
  system temporary directory.
- Returns the tmux session name and both file paths immediately so pi remains available for input.
- Subscribes to a per-command `tmux wait-for` completion channel and starts an immediate continuation
  when tmux signals completion, so pi does not poll or wait for a previously chosen timeout.
- Never places completion in Pi's `followUp` queue. While Pi is busy, it displays the structured task
  name through a persistent widget above the editor, retains the continuation internally, and starts
  a normal user turn on `agent_settled`. This removes the hard-coded `Follow-up:` TUI prefix.
- Names successful completion as
  `MM-DD HH:mm → HH:mm | tmux=<session> | <command>` without `exit=0`. A nonzero result adds
  `failed=<code>`. Command whitespace is collapsed and the identity is capped at 160 characters.
- Persists `launch.json` beside every job and records each launch as a versioned custom Pi session
  entry. `/reload` restores same-process artifacts; later resume of the same Pi conversation restores
  launches from session history even under a new PID. Both paths re-subscribe waiters and immediately
  reconcile existing `exit-status` files. A `completion-delivered` marker prevents duplicate
  continuation, and ID allocation scans every same-process artifact—including delivered jobs—so
  reload cannot reuse a session name.
- Defers completion that arrives during session compaction, then delivers it after compaction or
  `agent_settled`. Compaction events reconcile tracked exit-status files without interval polling.
- Uses one shared temporary-directory timestamp to allow at most one cleanup scan per 24 hours across
  reloads and Pi sessions. Cleanup examines artifacts sequentially, stats `exit-status` first, reads
  content only after the seven-day age threshold, and removes one directory at a time. It never uses
  unbounded `Promise.all`, never scans hourly, preserves active/incomplete jobs, and its unreferenced
  timer does not keep Pi running.

Detached jobs are not killed when a Pi process exits. Hot `/reload` restores immediately. If the
process is closed, the job cannot wake that closed process, but resuming the same Pi conversation
restores its recorded launch, reconciles completed status, and wakes the resumed context. Completed
output remains available
for seven days and is also cleaned on the next Pi startup if Pi was not running at the scheduled
cleanup time.

## Claudex integration

This package exports its long-command policy as
`@kkkaoru/pi-tmux-timeout-extension/policy`. `pi-claudex-provider` consumes that policy because its
Claudex gateway calls Pi providers directly without running Pi's agent loop. In that environment the
matching operation is delegated to Claude Code's native `run_in_background` Bash lifecycle instead
of starting another tmux session. Claudex's isolated Claude Code settings also run
`claudex-hook.ts` as a `PreToolUse` fallback, so the final Claude Code tool input is normalized even
when a routed provider omits the background flag. A Bash `PostToolUse` `asyncRewake` hook then watches
the native output file with `fs.watch`; completion wakes the exact originating Claude context with
task ID, exit status, output path, and a result-inspection request. It does not poll, `TaskStop`, or
control SubAgent lifetime. Normal standalone Pi usage continues to use `tmux_exec` and `tmux wait-for`
exactly as described above.

## Examples

Pi can call the tool directly:

```text
tmux_exec({ command: "gh run watch 32847265628 --exit-status --compact" })
```

A normal long-timeout bash call is rewritten automatically:

```text
bash({ command: "bun run check", timeout: 120 })
```

## Install

From the dotfiles root:

```bash
./create-symlinks.sh
```

This links the extension to `~/.pi/agent/extensions/tmux-timeout`. Restart pi or run `/reload`.

## Quality checks

```bash
bun install
bun run check
```
