# pi loop extension

A global [pi extension](https://pi.dev/docs/latest/extensions) for continuing work without
repeated manual prompts.

- `/loop <prompt>` runs immediately as a self-paced loop. The agent schedules another useful tick
  with `loop_wakeup`, and stops when complete or blocked.
- `/loop 5m <prompt>` and `/loop <prompt> every 5 minutes` run immediately and then recur on a
  fixed, session-scoped schedule. Supported units are seconds, minutes, hours, and days; intervals
  below one minute are rounded up.
- Bare `/loop` continues only work already established in the conversation.
- `/loop list` shows pending jobs; `/loop pause` freezes and persists their remaining delays;
  `/loop resume` restores the countdown from that exact remainder; `/loop clear` cancels and persists
  the empty state.

Loop continuations use the local-time naming line
`MM-DD HH:mm → HH:mm | loop=<id-or-self-paced> | <reason-or-task>`. The self-paced explanation stays
in the message body while its task identity is displayed through this naming line. When Pi is busy,
the extension shows every scheduled or ready name in a persistent widget above the editor, keeps the continuation
internally, and sends it as a normal user turn after `agent_settled`; it does not use Pi's `followUp`
queue or display its hard-coded queue prefix.

The self-paced behavior follows Codex's agent-loop principle: a turn continues through tool calls,
then the model explicitly decides whether another continuation is useful. A session-scoped
five-second background poller checks wall-clock deadlines, including overdue jobs after system
sleep. If pi compacts during an in-flight self-paced tick without retrying it, the extension retains
that tick internally so the loop continues from the compacted context. Every schedule, pause, resume,
fire, clear, and ready continuation writes a versioned custom session entry. On `/reload`, the newest
entry restores job IDs, absolute deadlines, paused remaining delays, pending continuations, and the
persistent widget. Overdue restored jobs fire immediately; paused jobs remain paused until `/loop resume`.
Pi's own retry and recurring jobs are left untouched to avoid duplicate runs. `loop_wakeup` uses parallel tool execution because it only
updates the in-memory schedule and does not need to serialize sibling tools. Polling starts only
after a command or tool schedules a job and stops when jobs are paused, cleared, or exhausted. Jobs
are session-scoped and persist across extension reloads and later resume of the same Pi session, but
do not migrate to an unrelated session.

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
