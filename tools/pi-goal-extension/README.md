# pi /goal

A session-scoped objective, defined by the user or agent from the user's established task,
that survives compaction and reload.
Inspired by Codex's `codex-rs/ext/goal` objective/state/continuation design (reference checkout
`c4017a87`); this is a
Pi implementation, not a complete port of Codex's TUI, attachments or accounting system.
Requires Pi 0.85.1 or newer. State validation uses Valibot.

## Commands

```text
/goal Implement the change and verify its tests
/goal --tokens 250000 Implement the change and verify its tests
/goal
/goal status
/goal pause
/goal edit Revised objective
/goal edit
/goal budget 500000
/goal budget none
/goal resume
/goal clear
```

- Startup and ordinary messages do not create goals implicitly. The user can use `/goal <objective>`;
  the agent can deliberately call `start_goal` when the established request benefits from durable
  completion tracking. This grants no new permissions and must not invent unrelated work.
- `start_goal` adopts the current run without submitting a duplicate prompt and creates no token
  budget. It refuses an active, paused or budget-limited goal. It may supersede a goal that is
  already **blocked**, because that blocker was already reported to the user and the replacement
  starts fresh accounting; only the user can resume stopped automation.
  Agent-defined goals use the same persistence, evidence, accounting and stall safeguards.
- `/goal <objective>` replaces an unfinished goal immediately, without a confirmation prompt. The
  replaced goal stays in session history and replacement resets usage; independent loops and
  detached processes are not stopped. No UI is required.
- No token budget is invented. `budget none` explicitly removes an existing limit.
- Editing the objective or budget leaves the goal paused; resume explicitly. Cancelling the
  goal editor also leaves it paused. Usage already consumed is not reset by editing.
- Pause/clear stop **future goal continuations** and invalidate queued goal prompts. They do
  not abort the current agent turn, cancel independent `/loop` jobs, or kill detached
  processes. Use Pi's abort control for the current turn and manage other jobs explicitly.
- `/goal` shows the full objective, status, usage and most recent audit/stop reason. The
  footer shows a compact status. Completed goals may be replaced by a new explicit goal.

## Agent tools and stopping

Four small schemas are registered: `start_goal`, `get_goal`, `update_goal`, and `goal_wait`. The detailed
objective guidance is added only when a goal exists; no additional skills or integration
catalogs are loaded. `get_goal` can return null. Blockers and waits require an active goal; a
verified completion audit is also accepted for a stopped goal, because closing it is bookkeeping
rather than resumption.

`update_goal` accepts a verified completion audit or a stable blocker reason. A completion audit is
recorded even when the goal stopped (manual pause, provider error, safe mode or exhausted budget)
and never resumes pacing; a blocker report still requires an active goal. The same
blocker must be reported on three consecutive agent runs before the goal becomes blocked;
repeated calls in one run do not advance that counter. A changed blocker or a gap resets
it. Three successful runs without tool activity, an explicit wait, a live owned task, or
pending notification also block continuation. These are conservative stall safeguards,
not proof that arbitrary tool activity advanced the objective. Completion remains an
agent audit; the extension does not independently prove all acceptance criteria.

Goal/control reads and pacing tools alone are not work evidence. Errors, aborts and empty
assistant responses pause the goal after Pi settles, rather than retrying indefinitely.
Pi's own retries/compaction may recover first: successful automatic context recovery keeps the
active goal running without manual `/compact`, `continue` or resume. A failed/cancelled compaction
puts an active goal in safe mode immediately, preventing goal pacing from bypassing a paused loop.
A failed retry after successful compression also stops automatically. Only the user can resume a
stopped goal; successful manual compaction never overrides a manual pause or safe-mode stop.

`goal_wait` schedules a justified recheck in 60–3,600 seconds. Prefer existing loop pacing
or live tmux completion notifications; do not create duplicate timers for the same wait.

## Loop and tmux cooperation

The three local extensions exchange small synchronous, session-ID-scoped activity
snapshots through Pi's event bus. The consumers import their local `src/goal-activity.ts`
source symlinks, which point to this package's dependency-free `src/activity.ts`. Keep those
Git-managed links intact: imports outside an extension directory resolve incorrectly when Pi
auto-discovers a symlinked `extensions/<name>/index.ts` entry point.
No global lock files or inferred task completion:

1. Non-paused loop work (running/pending continuations or scheduled jobs) owns pacing.
   Goal guidance remains available on those turns, but `/goal` does not send a competing
   continuation. Pausing/clearing an independent loop allows goal pacing to take over.
2. Pending tmux completion/overdue delivery takes priority. All launches made while the
   goal is active in the same session are tracked, including automatically detached bash.
   Pre-existing unrelated jobs do not stall a new goal.
3. Owned live jobs are monitored without model polling. Existing tmux notifications wake
   useful inspection; an estimate timeout is not completion and never justifies relaunching
   a duplicate job. Completion audits are refused while an owned job is live or its own completion
   or overdue notice is still undelivered; the refusal names those tasks, and a pending notice for a
   task the goal does not own never blocks it. Missing monitoring pauses rather than fabricating success.
4. Otherwise an idle check runs every five seconds, respecting compaction, UI prompts,
   pending messages, explicit waits and the active session. Only one goal prompt is pending
   at a time. Busy races retry later; a still-unaccepted prompt pauses for inspection at
   an idle check at least 30 seconds after submission. User input and changed goal revisions invalidate stale goal tickets.

This requires the corresponding loop/tmux changes in this checkout. Without those extensions,
standalone goals work, but external processes/agents not launched through this session's tmux
extension are not automatically monitored. Their handles and justified rechecks remain the
agent's responsibility. Pause/clear do not stop independent schedulers or external agents.

## Persistence and accounting

Custom `pi-goal-state-v1` entries are restored from the current **branch**, including across
compaction. Clear writes a tombstone. A malformed latest goal entry fails closed instead of
reviving an older active goal. A goal inherited into a different session is paused and loses
old process ownership until explicitly resumed. Subscriptions/timers are disposed on shutdown.

Token usage is Pi's reported `usage.totalTokens` (including cached tokens), accumulated from
assistant/tool result messages and compaction entries. Active runs count toward the goal;
manual compaction while active also counts. Elapsed time is agent-run wall time, not idle wait
or total time since goal creation. Provider errors may report incomplete/zero usage.

**Budgets are soft, checked at agent-run boundaries**, not per provider request: the current
run can exceed the budget. They stop new goal continuations, not independent loop jobs or
external processes. Detached agents' token usage is **not** attributed unless Pi reports it
in a tool result. No cost cap, external-agent budget enforcement, or billing accuracy is claimed.

## Install and verify

`create-symlinks.sh` includes the managed `goal` extension alongside `loop` and `tmux-timeout`.
Review that script before running it because it changes other live configuration too. Install
package dependencies with Bun, then `/reload` or start a new Pi session. This does not create
a goal or resume one in the current conversation automatically.

```sh
cd tools/pi-goal-extension
bun install --frozen-lockfile
bun run check
bun run smoke
bun run smoke:recovery
```

`smoke:recovery` uses the real Pi SDK with an offline scripted provider to verify agent-created
`start_goal`/`start_loop`, automatic overflow-to-emergency-compaction-to-task retry, preserved original
history, summary failure safe mode and exhausted-retry safe mode. It submits no recovery commands
and checks that no duplicate continuation arrives afterward. Only temporary and in-memory state is used.

`check` runs strict TypeScript, Biome and Vitest coverage (90% minimum). Unit tests mock clocks,
filesystem/process activity and Pi delivery. Also run `bun run check` in
`tools/pi-loop-extension` and `tools/pi-tmux-timeout-extension`.

`smoke` is a separate offline integration check: real Pi SDK, automatically discovered
symlinked `extensions/<name>/index.ts` entry points (not explicit package-directory loading),
in-memory credentials/catalog/session, isolated resources and a scripted fake provider.
Network requests are refused. It verifies pause/resume, actual goal tool execution/accounting,
native retry recovery, loop precedence, and one real disposable tmux job (`sleep 1; printf goal-smoke-ok`, five-second
hard limit), including notification and reading its exit status/output. It only removes its
own temporary directory; no real model, cloud integration or user job is used.
