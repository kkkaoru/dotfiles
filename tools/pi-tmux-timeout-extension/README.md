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
- Subscribes to a per-command `tmux wait-for` completion channel and queues an immediate follow-up
  turn when tmux signals completion, so pi does not poll and does not wait for a previously chosen
  timeout or `loop_wakeup` delay.

Detached jobs are not killed when a pi session exits. Completion monitoring is session-scoped, so a
job that finishes after pi exits cannot wake the closed session. Temporary output is intentionally
retained for later inspection and can be removed by the user after it is no longer needed.

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
