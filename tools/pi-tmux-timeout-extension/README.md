# pi tmux timeout extension

A global [pi extension](https://pi.dev/docs/latest/extensions) that keeps pi responsive while
long-running commands continue in detached tmux sessions.

## Behavior

- Registers the parallel `tmux_exec` tool for explicitly starting a command in detached tmux.
- Derives a 128-bit namespace from Pi's main session ID and uses a dedicated tmux server socket for
  that namespace. Session names are `pi-tmux-<namespace>-<counter>`, so different Pi sessions neither
  share names nor subscribe to each other's completion channels.
- Automatically rewrites `bash` tool calls with a timeout of at least 120 seconds.
- Automatically rewrites known blocking watchers: `gh run watch`, `watch`, and `tail -f`.
- Leaves short commands and commands that already invoke tmux unchanged.
- Stores combined stdout/stderr in `output.log` and the numeric exit code in `exit-status` under the
  system temporary directory.
- Returns the tmux session name and both file paths immediately so pi remains available for input.
- Shows every active task with its local start and estimated completion date/time in a persistent
  widget above the editor, and reports the active task count in the footer. Automatic Bash rewrites
  use the original timeout as the estimate; explicit `tmux_exec` calls accept
  `estimatedDurationSeconds`. Tasks restored after `/reload` or session resume are shown too.
- Reconciliation checks active tasks once per minute, removes orphaned tasks when both the tmux
  session and exit-status file are gone, marks them as `orphaned`, and prevents them from remaining in
  the widget indefinitely.
- Registers `/tmux-tasks [status|clear|hide|show|reset]` for session-scoped display control. `clear`
  persistently dismisses currently visible rows without stopping their jobs or completion monitoring;
  `hide` and `show` control the whole widget, and `reset` restores all tracked rows. The state survives
  `/reload` and session resume through a custom session entry.
- Subscribes to a per-command `tmux wait-for` completion channel and starts an immediate continuation
  when tmux signals normal completion; the minute reconciliation is only the orphan fallback.
- Sends completion continuations with Pi's supported `{ deliverAs: "followUp" }` delivery mode. While
  Pi is busy, it also shows a transient completion notification and retains the continuation internally
  until `agent_settled`. Delivery from that event is deferred by one event-loop turn so multiple
  settled handlers cannot all observe idle before an earlier asynchronous `sendUserMessage` call has
  activated or queued its run. Together with the explicit delivery mode, this prevents re-entrant
  prompt dispatch and a new prompt racing the final handoff. If several tasks finish while Pi remains
  busy, their eventual follow-up reports aggregate success/failure counts and only the newest task's
  command and artifact paths. All tasks are still marked delivered, so stale intermediate details do
  not flood a later agent turn or return after `/reload`.
- Names same-day completion as `HH:mm → HH:mm | <command>` and includes dates on both timestamps
  only when it spans local calendar dates: `MM-DD HH:mm → MM-DD HH:mm | <command>`. The tmux session
  identity remains in internal metadata and artifact paths but is omitted from completion displays.
  A nonzero result adds
  `command_exit=<code>` and is shown as a command-failure warning rather than a tmux extension error.
  Command whitespace is collapsed and the identity is capped at 160 characters.
- Persists `launch.json` beside every job and records each launch as a versioned custom Pi session
  entry. `/reload` and later resume restore only entries whose namespace matches the current main Pi
  session and whose artifact directory still exists, then re-subscribe through that session's
  dedicated tmux socket and immediately reconcile existing `exit-status` files. Entries whose
  temporary artifact directory was already removed are discarded instead of being reported as
  orphaned historical tasks. Runtime restoration rejects mismatched socket names, session names,
  and completion channels. A `completion-delivered` marker prevents duplicate continuation.
- Defers completion that arrives during session compaction, then delivers it after compaction or
  `agent_settled`. Compaction events also reconcile tracked exit-status files immediately.
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
