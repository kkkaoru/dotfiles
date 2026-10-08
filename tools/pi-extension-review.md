# Pi extension improvement review

## Scope and stopping rule

Review repository-managed Pi extensions against the installed and latest published stable Pi APIs.
Repeat research → hypothesis → minimal implementation → verification until the inventory has no
remaining evidence-backed actionable candidates. Do not add speculative features to prolong work.
Preserve unrelated edits, credentials, approval policy, global Pi installation and user workloads.
No commits/pushes or paid model probes are part of this review.

## Baseline

Previous migration: loop and tmux journals/waits use Pi Durable 1.0.4; native MCP connects to the
existing Executor bridge. The detached OS executor remains tmux. Unit checks passed for tmux (132),
loop (93), goal (101) and Executor (62), plus all three offline smoke scripts.
Existing working-tree changes include unrelated Apple Pro work and must remain untouched.

## Research and candidate ledger

| Candidate | Hypothesis / required evidence | Status |
| --- | --- | --- |
| Stable API and package inventory | Registry lookup confirms coding-agent and pi-durable latest stable = 1.0.4. Five affected providers/effort peers/dev SDKs aligned. Omlx keeps validated0.84 support and adds1.0.4 peers/dev baseline; goal already accepts1.0.4. | Verified |
| Lifecycle and durable reliability | Installed extensions.md forbids resources/timers in discovery factories. Tmux cleanup moved to session_start; deferred providers no longer use unowned timers; Web Access loads natively before lifecycle dispatch. Durable journal/lock/commit ordering and namespace recovery retained, no shell replay. | Verified |
| Provider and effort API compatibility | Fixed normalized system/tool replay, mixed-operation cache narrowing, branded direct-stream contexts, JSON arguments, test fixtures and Devin changed-instruction resets. Added parsed event hooks for Cursor/Devin. | Verified |
| Native feature simplification | Web Access now native. Keep goal completion/pause policy, loop scheduling, tmux OS isolation, dynamic effort, bounded/retained-request compaction, and omlx model-switch lifecycle: stable Pi defaults do not supply those exact behaviors. MCP already native via Executor. Do not adopt unpublished experimental/plugin entry points or rewrite functioning legacy provider registration into complete Provider objects merely for style. | Evaluated; no further justified replacement |

## Iterations

1. Started inventory and published-version lookup in a bounded detached job. No implementation
   changes at inventory time.
2. Found a concrete lifecycle violation: tmux cleanup ran at extension discovery, including the
   native loader smoke. Moved cleanup activation to session_start; constructor is inert and
   shutdown is repeat-safe. Added assertions for no scheduling before start, repeated start/stop,
   and restarting after shutdown. Source: installed Pi 1.0.4 docs/extensions.md, "Respect the
   runtime lifecycle". Published-version evidence: inventory job ending `-28`.
   Verification: job `-30`, all tmux checks and offline smoke pass. The first six-package
   API probe failed due to temporary tsconfig type-root resolution, not extension API errors;
   rerun with explicit existing node/vitest type paths before drawing conclusions.
3. Corrected probe `-31` confirms the provider issues above. Installed custom-provider.md says
   streams receive normalized TranscriptContext, with system prompt/tools inside system messages;
   use getCurrentSystemPrompt/getCurrentTools or collapseSystemMessages. This is a runtime fidelity
   gap, not merely a peer-version warning. Begin five-package dev dependency alignment in job `-32`;
   global Pi remains untouched. Provider stream-hook support and continuation prompt changes also
   require review while fixing normalization.
4. Cursor normalized prompt/tool replay and Devin initial prompt replay now pass all component
   checks (67 and 56 tests, job `-34`). ClinePass cache filtering needs an explicit chat type
   predicate, corrected. Claudex normalizes direct provider context and validates JSON tool input;
   new unit tests added. Their next check is `-35`. Effort's fixture typing is fixed, but formatting
   exposed existing over-limit files (context-guard source 348 vs340, tests572 vs500); split cohesive
   helpers/tests without changing compaction behavior or weakening lint. Peer metadata updates,
   Devin continuing-session system updates, and remaining API/lifecycle audit remain actionable.
5. Job `-36` ran five components: Cursor67, Devin57, ClinePass108, Claudex99,
   effort75 unit tests passed. Correction: effort file-level coverage was not passing; the
   later strict final sweep exposed the earlier misleading aggregate success. Devin now replays the full transcript in a fresh ACP session only when current system
   instructions change; unchanged instructions retain efficient continuation. Peers and local dev
   SDKs align to1.0.4. Effort helpers/tests split without changing compaction behavior.
6. Remaining lifecycle audit: Claudex server already starts/stops on session lifecycle; omlx starts
   only on model switches (CLI initial model covered by scripts/pi). Goal/loop have separate ownership
   policy and cannot be replaced just by generic Durable tasks. Found another factory-timer issue:
   lazy external loader loads factories 250ms after native discovery, so new session_start handlers
   (notably web-access) miss the initial event; custom Jiti aliasing also bypasses native loading.
   Evaluate native pinned package declarations for installed versions commandcode0.5.1,
   antigravity0.3.0, web-access0.24.0 with offline loader validation before changing configuration.
   Their startup latency and factory I/O must be checked; do not blindly remove behavior.
7. Job `-37` native-loads all three external packages offline successfully. Web Access moved to
   pinned native package declaration; custom wrapper deleted. The two providers remain deferred,
   but now initialize once at awaited session_start rather than an unowned factory timer. This may
   wait for provider catalog initialization at session start, intentionally preventing registration
   races, while discovery-only loads stay inert. Job `-38` offline lifecycle smoke passes.
8. Current provider docs require parsed stream instrumentation. Added awaited onProviderStreamEvent
   forwarding at Cursor SDK delta and Devin ACP update boundaries (not invented HTTP events), with
   ordering regressions. Raw HTTP onPayload/onResponse hooks are not exposed by either agent SDK;
   document that boundary rather than pretending to instrument wire requests. Five README baselines
   and migration steps now match Pi1.0.4. Job `-39` passes Cursor68/Devin58 tests; Devin mock callbacks
   needed awaited calls to remove lint warnings. Corrected those rather than silencing warnings.
9. Durable/lifecycle review found no further concrete supported changes: tmux state is session-wide
   by design, not branch-local; recovered jobs remain namespace checked and never re-executed.
   Loop and goal deliberate pause/approval boundaries must remain separate from generic task storage.
   Omlx's current event API passes installed type checks; add1.0.4 to its peer range and verify with
   current dev SDK, retaining older validated support. Final complete inventory verification next.
10. Strict final job `-40` passes tmux/loop/goal then correctly stops on effort coverage: multipart
    request callbacks, tiny-budget return and empty retained-request native fallback lacked tests.
    Added those three regression cases without changing implementation or thresholds. Job `-41`
    retries effort and the remaining packages/smokes; all pass with exit0.
11. Final native loader probe `-42` loads Cursor, Devin, ClinePass, Claudex, effort and omlx through
    installed Pi with networking disabled and a disposable agent directory. Each loads exactly one
    extension with zero errors; exit0. No remaining evidence-backed actionable candidates in scope.

## Final verification and handoff

- `-40`: tmux132, loop93, goal101 component checks pass before the now-fixed effort coverage failure.
- `-41` (exit0): effort78, Cursor68, Devin58, ClinePass108, Claudex99, omlx34, Executor62;
  all component checks pass, including their configured lint/type/coverage gates. Total833 unit tests.
- `-41`: all four offline smoke suites pass (tmux, loop, goal, deferred loader), along with
  Executor OAuth/setup shell fixtures. Real tmux smoke commands use isolated test jobs only.
- `-42` (exit0): six native provider/extension loader probes pass; Web Access native loading is
  also covered by the deferred-loader smoke.
- `git diff --check` passes. No global Pi update, credentials/approvals changes, live model workloads,
  commits or pushes. Unrelated Apple Pro working-tree changes preserved.
- Restart Pi (recommended) or `/reload` to activate changed code and package declarations; existing
  sessions retain their old loaded tool descriptions. Review remains pinned to verified stable1.0.4,
  not a promise of compatibility with future releases. The boundaries below are intentional deferrals.

## Remaining boundaries (not unimplemented required fixes)

- Raw HTTP payload/header instrumentation is unavailable behind Cursor SDK and Devin ACP; parsed
  event instrumentation is supported instead. No fake response objects or payload-rewrite claims.
- Full native replacement of the remaining two deferred provider loaders would force CommandCode's
  live-before-cache catalog fetch into discovery. Retain discovery laziness until upstream supports
  cache-only initialization; no speculative fork or new lifecycle framework.
- Native full-Provider registration, tool annotation/outputSchema additions and experimental plugins
  are not required to fix an observed issue here; preserve current public tool names/behavior.
- No live authenticated model runs are needed or authorized for this review. Offline mocks, native
  loaders and existing real-tmux SDK smoke scripts verify the compatibility boundaries.
