# pi loop extension

A global [pi extension](https://pi.dev/docs/latest/extensions) for continuing work without
repeated manual prompts, backed by `@earendil-works/pi-durable` (pinned to 1.0.4).
Requires Pi 1.0.4 or newer. Existing commands and tools are unchanged.

- `start_loop` lets the agent autonomously define a self-paced loop grounded in the user's established
  task. It adopts the current turn without injecting or queuing another initial prompt, then uses
  the same `loop_wakeup`/`loop_complete` decisions and persistence as `/loop`. Starting one supersedes
  existing loop jobs and retained ticks, including the leftover jobs of a paused loop, so a pause
  never blocks a new authorized task; the discarded job count is reported. It still refuses empty
  tasks. It grants no permissions and must not resume a paused goal in place; replacing a stopped
  goal with a fresh agent-defined objective is the supported path for continued work.
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
either decision, `agent_settled` continues that tick once. A second settle without `loop_wakeup` or
`loop_complete` stops the loop instead of replaying it forever across sessions. Deferring by one event-loop turn prevents re-entrant prompt
dispatch when multiple settled handlers observe idle before an earlier asynchronous
`sendUserMessage` call has activated or queued its run. Every `loop_wakeup` tick reapplies the
self-paced decision instructions around the saved task prompt, so later turns do not depend on the
model copying those instructions into its own wakeup prompt. A session-scoped
five-second Pi Durable task checks wall-clock deadlines, including overdue jobs after system
sleep. Its wait deadline is checkpointed; Pi Durable owns the waiting task rather than a
JavaScript interval in the installed extension. If pi compacts during an in-flight self-paced tick without retrying it, the extension continues
that tick once from the compacted context. Every schedule, pause, resume,
fire, clear, and ready continuation commits a Pi Durable document before acknowledging tools
or dispatching prompts. A custom session entry is mirrored after the commit for compatibility
and forks. On `/reload`, the durable document restores jobs, queued follow-ups, and in-flight ticks;
old session entries are imported only when no durable document exists. If the loop is
not paused, overdue jobs fire and ready continuations are delivered. If it is paused, the widget
stays and a warning tells you to `/loop resume` or `/loop clear`. Live ticks still continue once
without a terminal tool, then stop.
Pi's own retry and recurring jobs are left untouched to avoid duplicate runs. Tool acknowledgements
await the serialized durable commit queue; state changes remain synchronous within a turn.
Polling starts only
after a command or tool schedules a job and stops when jobs are paused, cleared, or exhausted. Jobs
are session-scoped and persist across extension reloads and later resume of the same Pi session, but
do not migrate to an unrelated session.

## Durable storage and boundaries

Each persisted Pi session uses a sibling `<session-file>.loop-durable/` directory, with
fsync-enabled JSONL storage and an exclusive `proper-lockfile` lock. Keep it with the session
when backing up or moving it. No-session SDK runs use MemoryStorage and cannot survive exit.
A lock conflict, corrupt durable state or storage failure fails closed; it does not silently
fall back to stale session entries. Fix the storage issue and reopen the session to recover.
Do not delete a live lock. An abandoned process lock expires through proper-lockfile's stale
lock handling, not by forcibly taking ownership.

Pi Durable owns loop state and timer tasks, not a second model agent. Model execution, tools,
permissions, providers and MCP remain in the existing Pi coding-agent session. Closing Pi stops
execution; reopening that session recovers it. This is not a background daemon. The bridge to
`sendUserMessage` is not an atomic cross-runtime transaction: an in-flight continuation can be
replayed after a crash. Do not treat it as exactly-once delivery or blindly retry side effects.

## Failure safety

Failed or aborted compaction pauses all loop scheduling immediately, including pending and
self-paced continuations. An assistant error/abort also pauses loops once Pi settles (a successful
native retry can recover first). Paused state is persisted across reloads. Settled delivery checks
both pause state and actual idleness; it never treats a failed turn as unfinished successful work.
Pi first attempts automatic compression and retry, including the effort-manager package's bounded
and emergency recovery. Successful recovery continues the active loop automatically; no manual
`/compact`, `continue` or resume is needed. Only failed recovery enters the paused safe mode.
After that stop, use `/compact` to recover context, then explicitly `/loop resume`; resuming without
resolving the failure will pause again. Successful manual compression does not resume a paused loop.
A new `start_loop` supersedes such a paused loop instead of waiting for `/loop resume`, and reports
the discarded jobs. `/loop pause` also suppresses already-retained continuations. A settled tick
without a terminal tool call abandons the loop only when nothing owned is unfinished; while
loop-owned detached launches (tracked through the session activity bus) or running/pending
continuations remain, pacing re-arms for up to 10 consecutive extensions. `loop_complete` and
`clear` reset that budget, and unrelated tmux jobs never extend a loop.

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

This links the extension to `~/.pi/agent/extensions/loop`. First run `bun install` in this
component, then restart pi or run `/reload`. Existing paused loops remain paused on migration.

## Quality checks

```bash
bun install
bun run check
bun run smoke
```

Vitest enforces 95% minimum branch, function, line, and statement coverage. Oxlint enables every
rule category and type-aware checks; Oxfmt is the sole formatter. The offline smoke test
uses the real Pi SDK, JSONL storage and exclusive lock in a disposable directory; it makes
no model requests or network calls.
