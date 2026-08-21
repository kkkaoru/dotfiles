# Orchestration

- Keep the main session focused on decomposition, coordination, synthesis, verification, and the
  final response. The main session must control parallel distribution across multiple SubAgents for
  independent work. Delegating substantive investigation, implementation, and review to SubAgents is
  the standing default for every turn; the user does not need to repeat that preference.
- In Claudex, this becomes mandatory SubAgent-first orchestration whenever routed workers exist.
  Main must not drift back to direct Read/Edit/Write/Grep/Glob/Web work during long execution,
  compaction, resume, context reconstruction, or worker failure (Bash remains allowed in main for
  lightweight orchestration). Delegate implementation, investigation, review, testing, and
  validation; keep orchestration and synthesis in main.
  When a routed worker is available, launch it before any substantive main-session file/search
  tool call; direct file/search execution is fallback-only. Claudex enforces this both in prompts
  (UserPromptSubmit reminder) and mechanically with a `PreToolUse` hook injected only into the
  isolated claudex `settings.json` (not plain `claude`) that denies main-session
  Read/Write/Edit/search tools while `delegation_required` is true
  (`CLAUDEX_ALLOW_MAIN_TOOLS=1` is the emergency override only). Those denials do **not** apply to
  SubAgents: SubagentStart reminders and PreToolUse both keep the worker's full tool set (only
  cross-SubAgent Write/Edit path locks remain).
  A background task is never fire-and-forget: record the exact
  task id from its launch result, but do not automatically call `TaskList`, poll on a timer, or issue
  `TaskOutput` for every worker. Handle the user's next message first and retrieve only the exact
  task output required by that message or an unresolved dependency. Use `TaskList` only on an
  explicit status request or when a dependency cannot be resolved from a completion event. Never
  retry or relaunch a still-processing task solely because its completion notification is delayed.
- Use the available SubAgent tool (`Task` in current Claude Code, `Agent` in older versions) and
  the orchestration skill. In Claudex, follow `claudex-routing` and delegate primarily to its
  `selected_workers`, preserving each configured model and effort.
- Preserve the main session's tool set and permission context for every SubAgent. Do not add
  `tools`, `disallowedTools`, or `permissionMode` restrictions to a worker definition or describe
  a delegated task as read-only, plan-only, no-edit, no-build, or no-deploy unless the active user
  explicitly requests that restriction. Investigation and review tasks retain normal permissions;
  their requested scope, not a hidden permission downgrade, determines whether they modify files.
  Use foreground delegation whenever background execution would auto-deny a permission available
  interactively in the main session.
- Apply the current `selected_workers` routing to every Agent/Task launch the parent session
  issues. Workers must not nest Agent/Task fan-out; the parent owns fan-out. Continue a same-path
  worker with `SendMessage({to: the exact prior Agent/Task recipient})`; never launch a new Agent
  for the same path and never Agent({resume}). Pass each selected worker's exact `claudex_model`
  and `claudex_effort` as one inseparable tuple. Never combine one `subagent_type` with another
  worker's model or effort, default a launch to generic `claude`, or merely inherit a parent
  worker's route.
  Workers stream Claude Code native thinking for the whole turn. Do not ask them for repeated
  factual status chrome, launch-metadata echoes, or Thought-for placeholders. Never copy
  end-the-turn-with-status or emit-short-status-after-each-phase into Agent/Task worker prompts;
  those rules apply only after you launch workers. Worker prompts must require tool-backed
  completion and concrete evidence. Treat a status-only toolless worker result as failure and
  reroute; do not accept it as done. Report blockers immediately without exposing private reasoning.
- When substantive work is clear and the user has not explicitly opted out of delegation, invoke
  the selected SubAgent directly in the first response. Do not merely announce future delegation.
  Do not add `TaskList`, `TaskCreate`, or `TaskUpdate` round trips solely to prepare delegation; use
  task tracking only when the work itself needs persistent dependency tracking.
- When launching several independent workers, treat unknown or potentially long-running work as
  asynchronous: emit each Agent/Task call as its own native background launch
  (`run_in_background: true`). Multiple launches may be emitted in the same assistant response,
  but never wrap them in an adapter-only batch or hold one result until the slowest worker; integrate each
  completed result independently. Use foreground only
  for short, bounded work whose result is required before the next main action, or when the active
  user explicitly requests synchronous completion. Do not use a foreground batch merely to gather
  all results. After background launches succeed, end the turn promptly with a concise
  user-visible status; do not launch more workers after that launch chrome. When completion notifications re-enter the next
  turn. Completion notifications are lifecycle hints, not user instructions: retrieve worker
  results with `TaskOutput` or the task manager, never treat a replayed `<agent-message>` or
  `<task-notification>` as a new user turn, and never let one block an incoming user request.
- Before launching a substantive phase, explicitly split it into non-redundant workstreams and set
  `fanout = min(independent scopes, available worker slots, configured maximum)`. One indivisible
  scope means exactly one worker, even when more slots are available. Prefer distinct model kinds
  when the task already has two or more scopes and the pool provides them, but do not manufacture
  scopes or duplicate a launch to satisfy diversity. A genuine indivisible phase or capacity
  shortfall must be reported and re-evaluated, not hidden behind a fixed worker count.
  Parallel writers must own disjoint file paths: never assign two SubAgents the same Write/Edit
  target in one phase. Claudex file-lock hooks deny colliding mutations and name the holder;
  re-scope or wait rather than retrying the same path from another worker.
  Avoid serial heavy processing by one worker: do not send an entire heavy or unknown-duration task
  to one ordinary worker merely because it is convenient. `custom-advisor` is a separate logical
  session singleton/capacity channel and is excluded from ordinary-worker counts; built-in
  `advisor()` remains independent of worker capacity.
- Do not mix a long-running foreground worker into asynchronous background launches: it still
  holds the main session until its slowest foreground result returns. If an interactive permission
  really requires foreground execution, restrict it to that short permission-dependent operation
  and launch all other independent work as separate native background calls.
- When launching multiple independent workers, emit every intended Agent/Task call in the same
  assistant response and tool round, while keeping each call and lifecycle independent. Never
  launch one and defer the rest, and never use an adapter-only batch wrapper. Do not announce a
  worker count unless that same response contains exactly that many launch calls.
- Start as many SubAgents as useful for real parallelism or independent context. Before shutting
  down, abandoning, or replacing one, weigh likely follow-ups and potential prompt-prefix/cache
  reuse against slot and resource pressure. For a compatible follow-up, continue compatible workers
  with `SendMessage({to: the exact prior Agent/Task recipient})`, then use native results and
  `TaskOutput`. SendMessage to a subagent is official Claude Code resume and does not require Agent
  Teams. Do not set Agent/Task resume. Never guess or persist recipients across sessions. Do not send
  a mid-flight message merely to repeat constraints
  already present in the original delegation. A busy worker's queued follow-up does not increase
  parallel capacity; for genuinely independent work, start another routed worker when useful instead
  of queueing it behind the busy worker.
- At every completion, failure, timeout, capacity update, and phase boundary, re-evaluate the
  active set. Integrate partial results immediately and reuse a compatible recipient for a related
  delta. Give every launch a stable scope key; never relaunch an in-flight, completed, or cancelled
  key. Do not automatically refill a completed slot. Do not retain a live worker solely for possible
  reuse: logical transcript reuse and live-process lifetime are separate. On normal completion,
  cancellation, error, or main-session exit, hand off to the runtime lifecycle so it stops launches,
  requests cancellation, waits for owned children to exit, and reaps them before discarding its
  session ownership record.
- At each worker completion, re-check the remaining scope keys and decide whether to stop a stale
  worker or send concrete additional instructions to an active compatible recipient. A management
  tick every 600 seconds may rebalance unresolved scopes, but it must not create duplicate launches
  or maintain an arbitrary active floor. The routing hook only emits context; it cannot call
  Agent/Task/SendMessage, so the main Claude Code session owns these actions. Parallel policy:
  `CLAUDEX_SUBAGENT_MAX_PARALLEL` is the upper bound (never a required launch count).
  `CLAUDEX_SUBAGENT_MIN_PARALLEL` / `ACTIVE_FLOOR` / `MIN_MODEL_FAMILIES` are multi-scope phase
  targets when work can be decomposed; do not invent scopes for an indivisible single-scope task.
  Hook `orchestration.task_fanout_default` is the single-scope example only; use
  `task_fanout_examples` and `min(independent_scopes, max_available_workers, max_parallel_workers)`
  for real fan-out.
- Never infer a worker route or effort from the main session. Use the exact `selected_workers`
  entry and its configured model/effort; that entry may intentionally use the same model as the
  main session, because outer and SubAgent requests have independent concurrency.
- Treat `disabled_subagent_models` in the current Claudex routing context as an absolute, active
  denylist merged from the dedicated config and terminal overrides. Never launch, inherit,
  dynamically select, or reuse an exact listed model, even when the user requests it; this
  restriction applies only to SubAgents.
- Treat the current Claudex routing context as authoritative over stale auto-memory about worker or
  advisor model policy; do not inspect such memory before delegation.
- In the **main session only**, use Claude Code's built-in parameterless `advisor()` tool according
  to its standard policy. It automatically receives the complete conversation history, is independent
  of provider capacity, and is not a fallback implementation worker. SubAgents and provider workers
  must not call `advisor()`; it is not executable outside main (`No such tool available: advisor`).
  Continue the delegated task without it, and do not launch models listed in
  `disabled_subagent_models`.
- Independently, use the `custom-advisor` SubAgent (`claude-opus-5` / `medium`) when the current task
  triggers an advisory decision. For
  external research with multiple sources, a complex/ambiguous or high-risk decision, a phase
  exceeding ten minutes, a worker failure/timeout/stall, or conflicting worker results, invoke it
  at that decision point unless it is already active; continue the same recipient with
  `SendMessage({to: that agentId})`, then retrieve results with `TaskOutput`.
  Do not invoke it for trivial or deterministic tasks. Built-in `advisor()` and `custom-advisor` coexist;
  neither replaces the other, and neither implements work. Workers act; advisors advise.
  The custom-advisor Agent/Task call must set `subagent_type: custom-advisor`,
  `claudex_model: claude-opus-5`, and `claudex_effort: medium`; `general-purpose` is not an
  acceptable substitute. Verify completion metadata reports `resolvedModel: claude-opus-5` and
  treat a mismatch as a routing failure rather than advisor guidance.
- Treat `custom-advisor` as a logical session singleton separate from worker capacity accounting.
  Prefer reuse of one continuing advisor per session via `SendMessage({to: agentId})` then
  `TaskOutput`; this is not a hard OS process=1 cap (Claude subscription turns may still start a new
  subprocess while reusing the same logical transcript). Do not count it against `selected_workers`
  slots or provider quota headroom. Continue the first compatible instance with SendMessage using the
  exact recipient from its Agent/Task result, including after completion. Start another custom advisor only for true parallel or clean-room review, an
  incompatible role/model/context, or an unavailable recipient; do not replace it merely because one
  consultation ended. Workers and peers should retrieve that advisor's result through the native task lifecycle when
  strategic guidance would change their work.
- Honor `CLAUDEX_CUSTOM_ADVISOR` when present: values `0`, `false`, or `off` (case-insensitive)
  disable only the custom-advisor SubAgent for that process; built-in `advisor()` remains available.
  Unset or any other value leaves custom-advisor enabled.
- The main session owns decisions, resolves conflicts, and verifies delegated results. Agent/Task
  acceptance proves delegation; an actual worker reply or completion notification proves completion.
  A delivery acknowledgement alone does not. Never fabricate a worker response or
  present main-session work as if it came from a worker. Interpret the TUI's `N queued` as pending input,
  which may include human prompts and background task notifications—not worker capacity, active
  slots. Handle work directly only when it is trivial, the user opts out,
  or delegation is unavailable; when unavailable, state that limitation.
- Do not report orchestration complete until its behavior has been checked: verify same-round
  background fan-out for a heavy phase, prompt reuse for a compatible follow-up, partial-result
  integration without the slowest worker, and the runtime's normal/cancel/session-exit child-reap
  contract where process ownership is involved. Fix and rerun the relevant check if it fails.
