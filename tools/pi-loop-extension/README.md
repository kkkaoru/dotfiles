# pi loop extension

A global [pi extension](https://pi.dev/docs/latest/extensions) for continuing work without
repeated manual prompts.

- `start_loop` lets the agent autonomously define a self-paced loop grounded in the user's established
  task. It adopts the current turn without injecting or queuing another initial prompt, then uses
  the same `loop_wakeup`/`loop_complete` decisions and persistence as `/loop`. Starting one supersedes
  existing unpaused loop jobs and retained ticks, so leftover work never blocks a new agent task.
  It still refuses empty tasks and paused loops. It grants no permissions and must not bypass a
  paused goal or safe mode.
- `/loop <prompt>` runs immediately as a self-paced loop. The agent must finish immediately
  actionable work, schedule a useful later tick with `loop_wakeup`, or explicitly stop with
  `loop_complete` only when complete or blocked on user input.
- `/loop 5m <prompt>` and `/loop <prompt> every 5 minutes` run immediately and then recur on a
  fixed, session-scoped schedule. Supported units are seconds, minutes, hours, and days; intervals
  below one minute are rounded up and intervals above 30 days are rejected.
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
Pi's own retry and recurring jobs are left untouched to avoid duplicate runs. `loop_wakeup` uses
parallel tool execution because its schedule, state and persistence updates are synchronous and it
does not need to serialize sibling tools. Polling starts only
after a command or tool schedules a job and stops when jobs are paused, cleared, or exhausted. Jobs
are session-scoped and persist across extension reloads and later resume of the same Pi session, but
do not migrate to an unrelated session.

## Failure safety

Failed or aborted compaction pauses all loop scheduling immediately, including pending and
self-paced continuations. An assistant error/abort also pauses loops once Pi settles (a successful
native retry can recover first). Paused state is persisted across reloads. Settled delivery checks
both pause state and actual idleness; it never treats a failed turn as unfinished successful work.
Pi first attempts automatic compression and retry, including the effort-manager package's bounded
and emergency recovery. Successful recovery continues the active loop automatically; no manual
`/compact`, `continue` or resume is needed. Only failed recovery enters the paused safe mode.
After that stop, use `/compact` to recover context, then explicitly `/loop resume`; resuming without
resolving the failure will pause again. Successful manual compression does not resume a paused loop. `/loop pause` also suppresses already-retained continuations.

## Goal cooperation

With the local `pi-goal-extension`, non-paused loop work owns continuation pacing. The goal
adds durable objective guidance without creating a competing wakeup. Session-scoped activity
queries report running/pending continuations and scheduled jobs; subscriptions are removed on
shutdown. Pausing or clearing a goal does not stop independent loops; manage them with `/loop`.
See `../pi-goal-extension/README.md` for the complete behavior and offline integration check.

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
