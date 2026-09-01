# pi loop extension

A global [pi extension](https://pi.dev/docs/latest/extensions) for continuing work without
repeated manual prompts.

- `/loop <prompt>` runs immediately as a self-paced loop. The agent must finish immediately
  actionable work, schedule a useful later tick with `loop_wakeup`, or explicitly stop with
  `loop_complete` only when complete or blocked on user input.
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
the extension shows every scheduled or ready name in a persistent widget above the editor, keeps the
continuation internally, and sends it after `agent_settled` with Pi's supported
`{ deliverAs: "followUp" }` mode. This remains race-safe if another prompt starts between the idle
check and delivery, preventing repeated `<runtime>` busy errors.

The self-paced behavior follows Codex's agent-loop principle: a turn continues through tool calls,
then the model explicitly chooses exactly one terminal action. `loop_wakeup` schedules a later check;
`loop_complete` ends the loop only after completion or a user-input blocker. If a tick ends without
either decision, `agent_settled` schedules the retained task for the next event-loop turn instead of
silently ending with residual work. Deferring by one event-loop turn prevents re-entrant prompt
dispatch when multiple settled handlers observe idle before an earlier asynchronous
`sendUserMessage` call has activated or queued its run. Every `loop_wakeup` tick reapplies the
self-paced decision instructions around the saved task prompt, so later turns do not depend on the
model copying those instructions into its own wakeup prompt. A session-scoped
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
