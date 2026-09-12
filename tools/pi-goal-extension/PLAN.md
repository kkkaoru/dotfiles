# pi /goal implementation plan

## Result

Implemented and verified against Pi 0.85.1. Current checks passed: goal 81 tests,
loop 43 tests, tmux 78 tests; typechecks, lint, coverage gates, shell syntax and
`git diff --check`. The offline native SDK smoke also passed symlink loading,
pause/resume, accounting, native retry recovery, loop precedence and a disposable
real tmux job's notification/evidence audit. The managed goal symlink is installed;
existing sessions still need `/reload`. No commit or push was made.

See README.md for implemented scope and limits, especially soft per-agent-run budgets,
external-agent attribution, and pause/clear affecting future goal continuations only.
The sequence below records the design plan, not outstanding work.

### Startup-path regression correction

The original smoke explicitly loaded package directories, which canonicalized their paths and
missed Pi's normal discovery of symlinked `extensions/<name>/index.ts` files. Loop/tmux now
import extension-local `src/goal-activity.ts` source symlinks rather than traversing outside
their logical extension directories. The shared implementation remains dependency-free and
single-source. The smoke now uses automatic discovery and asserts the exact logical entry-point
paths before exercising the SDK. All 202 tests, type/lint checks and the corrected native SDK
smoke passed; both live extension-local links resolve to the canonical shared source.

## Reference

Read-only reference checkout: `/Users/kkk4oru/ghq/github.com/openai/codex`.
Relevant sources: `codex-rs/ext/goal/src/{spec,runtime,steering,accounting}.rs`,
`codex-rs/ext/goal/templates/goals/continuation.md`, and
`codex-rs/tui/src/{goal_display,chatwidget/slash_dispatch}.rs`.
Use the design, not copied proprietary credentials, database state, or live sessions.

## Behavior and boundaries

- A session-scoped persisted goal, created only by an explicit `/goal <objective>` command.
- `/goal` and `/goal status` show state. Support pause, resume, clear, edit and explicit
  `--tokens <positive integer>` budgets. Do not silently replace an unfinished goal.
- States: active, paused, blocked, budget_limited, complete. Preserve the full objective.
- Agent tools: get_goal, update_goal (completion/blocker audit), goal_wait (bounded wait).
  Only the user can replace objectives, change budgets, or resume a stopped goal.
- Count reported Pi token usage, including nested tool/compaction usage when available;
  clearly document attribution limits for detached external agents. Budget limits stop new
  goal continuations, never forcibly kill shell work or pretend work is complete.
- Goal continuations include evidence-based completion and no-progress audits. Three consecutive
  turns reporting the same blocker are required before automatic blocked status; reset on resume.
- Restore session state after reload/resume and compaction; no unrelated-session inheritance.
  Forked goals should be paused until explicitly resumed. Clear and pause invalidate stale queued
  goal prompts. Respect aborts/errors; never resume a user-cancelled goal automatically.

## Coexistence

Use Pi's event bus for a small session-ID-scoped activity query protocol. Loop and tmux
extensions expose runtime snapshots, not guessed lock-file state. Subscriptions are started
on session_start and disposed on session_shutdown.

- Existing loop work (running ticks, pending continuations or scheduled non-paused jobs) owns
  pacing. A goal adds durable objective guidance but does not enqueue competing continuation.
- Pending tmux completion/overdue delivery has priority. Track tmux launches made while the
  goal is active via scoped launch notifications; pre-existing unrelated jobs must not stall it.
- Wait for owned live tmux jobs without model polling or duplicate commands. Existing tmux
  completion/overdue wakeups resume useful inspection. A bounded explicit goal_wait can schedule
  a recheck of other external work. Missing monitoring is not fabricated completion.
- Use followUp delivery only after idle/settled checks, with stale goal-ID/revision tickets
  intercepted before provider execution. Do not change another extension's user-owned jobs.
- Pause/clear stop goal scheduling, not independent loops or detached processes. Explain this
  distinction in status/help and retain user control of those jobs.

## Implementation sequence

1. Add a small independent `tools/pi-goal-extension` package: typed state/parser, persistence,
   prompt generation, activity protocol, runtime/controller, command/tool registration.
2. Add optional activity providers to loop and tmux extensions. Preserve their behavior when
   goal is absent. Add the global extension symlink through create-symlinks.sh.
3. Deterministic mocked unit tests for state transitions, budgets, blocking audits, reload,
   queued stale prompts, aborts, compaction/retry and cross-session isolation.
4. Cross-extension tests for loop precedence, tmux live waits/completion/overdue, simultaneous
   settled callbacks, and pause/clear while queued. No real cloud calls or user job termination.
5. Run tsc/lint/format/test/coverage for each changed package (>=90%, preserving existing stricter
   gates). Run an isolated Pi SDK smoke with a fake provider and actual extension loading, not a
   paid model or user session. Verify shell syntax and git diff --check.
6. Update README and CLAUDE.md usage. Preserve all pre-existing working-tree changes. No commit
   or push unless requested. Finish the active self-paced loop only after verification completes.
