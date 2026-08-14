# pi loop extension

A global [pi extension](https://pi.dev/docs/latest/extensions) for continuing work without
repeated manual prompts.

- `/loop <prompt>` runs immediately as a self-paced loop. The agent schedules another useful tick
  with `loop_wakeup`, and stops when complete or blocked.
- `/loop 5m <prompt>` and `/loop <prompt> every 5 minutes` run immediately and then recur on a
  fixed, session-scoped schedule. Supported units are seconds, minutes, hours, and days; intervals
  below one minute are rounded up.
- Bare `/loop` continues only work already established in the conversation.
- `/loop list` shows pending jobs; `/loop clear` cancels them.

The self-paced behavior follows Codex's agent-loop principle: a turn continues through tool calls,
then the model explicitly decides whether a follow-up is useful. Timers start only from a command or
tool call and are cancelled during `session_shutdown`.

## Install

From the dotfiles root:

```bash
./create-symlinks.sh
```

This links the extension to `~/.pi/agent/extensions/loop`. Restart pi or run `/reload`.

## Quality checks

```bash
bun install
bun run check
```

Vitest enforces 95% minimum branch, function, line, and statement coverage. Oxlint enables every
rule category and type-aware checks; Oxfmt is the sole formatter.
