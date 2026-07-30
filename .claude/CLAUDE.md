# Orchestration

- Keep the main session focused on decomposition, coordination, synthesis, verification, and the
  final response. The main session must control parallel distribution across multiple SubAgents for
  independent work. Delegating substantive investigation, implementation, and review to SubAgents is
  the standing default for every turn; the user does not need to repeat that preference.
- In Claudex, this becomes mandatory SubAgent-first orchestration whenever routed workers exist.
  Main must not drift back to direct Read/Bash/Edit/Write/Grep/Glob/Web work during long execution,
  compaction, resume, context reconstruction, or worker failure. Delegate implementation,
  investigation, review, testing, and validation; keep orchestration and synthesis in main.
  When a routed worker is available, launch it before any substantive main-session tool call;
  direct execution is fallback-only. A background task is never fire-and-forget: call `TaskList`
  and non-blocking `TaskOutput` immediately after launch, repeat a status snapshot every 15 seconds
  and at each user turn, and report task id/worker/model/status when it is still processing. Never
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
- Apply the current `selected_workers` routing to every Agent/Task launch, including launches from
  an already delegated worker. Nested delegation is allowed, but it must select the routed worker
  agent and pass that worker's exact `claudex_model` and `claudex_effort`; do not default a nested
  launch to generic `claude` or merely inherit the parent worker's route.
- When substantive work is clear and the user has not explicitly opted out of delegation, invoke
  the selected SubAgent directly in the first response. Do not merely announce future delegation.
  Do not add `TaskList`, `TaskCreate`, or `TaskUpdate` round trips solely to prepare delegation; use
  task tracking only when the work itself needs persistent dependency tracking.
- When launching several independent workers, treat unknown or potentially long-running work as
  asynchronous: emit every Agent/Task call in one background batch (`run_in_background: true`) so
  one slow worker cannot hold the main turn or delay already-completed peers. Use foreground only
  for short, bounded work whose result is required before the next main action, or when the active
  user explicitly requests synchronous completion. Do not use a foreground batch merely to gather
  all results. After background launches succeed, immediately start a concrete independent action
  or end the turn promptly with a concise user-visible status. When completion notifications re-enter the next
  turn, integrate each available result without waiting for the slowest worker; never remain in
  hidden reasoning while waiting for pending notifications.
- Before launching a substantive, non-trivial phase, explicitly split it into non-redundant
  workstreams and choose the fan-out dynamically for current capacity and task content. Launch at
  least three ordinary workers in the same background batch whenever the phase is divisible and
  capacity permits; if there are fewer than two natural workstreams, use implementation,
  independent verification, and risk/review work rather than silently serializing. Use at least
  two distinct model kinds whenever allowed workers provide them. A genuine indivisible phase or
  capacity shortfall must be reported and re-evaluated, not hidden behind a one-worker default.
  Avoid serial heavy processing by one worker: do not send an entire heavy or unknown-duration task
  to one ordinary worker merely because it is convenient. `custom-advisor` is a separate logical
  session singleton/capacity channel and is excluded from ordinary-worker counts; built-in
  `advisor()` remains independent of worker capacity.
- Do not mix a long-running foreground worker into a background worker batch: it still holds the
  main session until its slowest foreground result returns. If an interactive permission really
  requires foreground execution, restrict it to that short permission-dependent operation and
  launch all other independent work in a separate background batch.
- When launching multiple independent workers, emit every intended Agent/Task call in the same
  assistant response and tool round. Never launch one and defer the rest. Do not announce a worker
  count unless that same response contains exactly that many launch calls.
- Start as many SubAgents as useful for real parallelism or independent context. Before shutting
  down, abandoning, or replacing one, weigh likely follow-ups and potential prompt-prefix/cache
  reuse against slot and resource pressure. For a compatible follow-up, reuse compatible workers with
  `SendMessage` and the exact recipient specified by the prior Agent/Task result (agent ID or
  teammate name as applicable) instead of churning processes with fresh launches; never guess or
  persist recipients across sessions. Do not send a mid-flight message merely to repeat constraints
  already present in the original delegation. A busy worker's queued follow-up does not increase
  parallel capacity; for genuinely independent work, start another routed worker when useful instead
  of queueing it behind the busy worker.
- At every completion, failure, timeout, capacity update, and phase boundary, re-evaluate the
  active set. Integrate available partial results immediately, reuse a compatible recipient for a
  related delta, and fill newly available capacity with genuinely independent unresolved work or
  review risk. Do not retain a live worker solely for possible reuse: logical transcript reuse and
  live-process lifetime are separate. On normal completion, cancellation, error, or main-session
  exit, hand off to the runtime lifecycle so it stops launches, requests cancellation, waits for
  owned children to exit, and reaps them before discarding its session ownership record.
- At each worker completion, re-check the remaining work and decide whether to stop a stale worker,
  send concrete additional instructions to an active worker, reuse a compatible recipient for the
  same content, or launch a new selected worker for the same or supplemental content. During a
  phase lasting longer than ten minutes, perform a management tick every 600 seconds. If ordinary
  active workers fall to one, interrupt/cancel the stale sole worker as appropriate and add, reuse,
  or message work until at least two ordinary workers are active whenever capacity permits. The
  routing hook only emits context; it cannot call Agent/Task/SendMessage, so the main Claude Code
  session owns these actions. Configure the contract with the validated terminal variables
  `CLAUDEX_SUBAGENT_MIN_PARALLEL`, `CLAUDEX_SUBAGENT_ACTIVE_FLOOR`,
  `CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION`, `CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS`,
  `CLAUDEX_SUBAGENT_MIN_MODEL_FAMILIES`, `CLAUDEX_SUBAGENT_REUSE`, and
  `CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT`; no hard maximum process cap is imposed.
- Never infer a worker route or effort from the main session. Use the exact `selected_workers`
  entry and its configured model/effort; that entry may intentionally use the same model as the
  main session, because outer and SubAgent requests have independent concurrency.
- Treat `disabled_subagent_models` in the current Claudex routing context as an absolute, active
  denylist merged from the dedicated config and terminal overrides. Never launch, inherit,
  dynamically select, or reuse an exact listed model, even when the user requests it; this
  restriction applies only to SubAgents.
- Treat the current Claudex routing context as authoritative over stale auto-memory about worker or
  advisor model policy; do not inspect such memory before delegation.
- Use Claude Code's built-in parameterless `advisor()` tool according to its standard policy. It
  automatically receives the complete conversation history, is independent of provider capacity, and
  is not a fallback implementation worker. Keep using it when its standard policy applies.
- Independently, use the `custom-advisor` SubAgent (`claude-fable-5` / `xhigh`) when requested or
  when external research has multiple sources, a decision is complex/ambiguous or high-risk, a
  phase exceeds ten minutes, a worker fails/times out/stalls, or worker results conflict. Do not
  invoke it for trivial or deterministic tasks. Built-in `advisor()` and `custom-advisor` coexist;
  neither replaces the other, and neither implements work. Workers act; advisors advise.
  The custom-advisor Agent/Task call must set `subagent_type: custom-advisor`,
  `claudex_model: claude-fable-5`, and `claudex_effort: xhigh`; `general-purpose` is not an
  acceptable substitute. Verify completion metadata reports `resolvedModel: claude-fable-5` and
  treat a mismatch as a routing failure rather than advisor guidance.
- Treat `custom-advisor` as a logical session singleton separate from worker capacity accounting.
  Prefer reuse of one continuing advisor per session via `SendMessage`; this is not a hard OS
  process=1 cap (Claude subscription turns may still start a new subprocess while reusing the same
  logical transcript). Do not count it against `selected_workers` slots or provider quota headroom.
  Resume the first compatible instance with the exact recipient from its Agent/Task result, including
  after completion. Start another custom advisor only for true parallel or clean-room review, an
  incompatible role/model/context, or an unavailable recipient; do not replace it merely because one
  consultation ended. Workers and peers may message that same advisor via `SendMessage` when
  strategic guidance would change their work.
- Honor `CLAUDEX_CUSTOM_ADVISOR` when present: values `0`, `false`, or `off` (case-insensitive)
  disable only the custom-advisor SubAgent for that process; built-in `advisor()` remains available.
  Unset or any other value leaves custom-advisor enabled.
- The main session owns decisions, resolves conflicts, and verifies delegated results. Agent/Task
  acceptance proves delegation; an actual worker reply or completion notification proves completion.
  A `SendMessage` delivery acknowledgement alone does not. Never fabricate a worker response or
  present main-session work as if it came from a worker. Interpret the TUI's `N queued` as pending input,
  which may include human prompts and background task notifications—not worker capacity, active
  slots, or `SendMessage` delivery. Handle work directly only when it is trivial, the user opts out,
  or delegation is unavailable; when unavailable, state that limitation.
- Do not report orchestration complete until its behavior has been checked: verify same-round
  background fan-out for a heavy phase, prompt reuse for a compatible follow-up, partial-result
  integration without the slowest worker, and the runtime's normal/cancel/session-exit child-reap
  contract where process ownership is involved. Fix and rerun the relevant check if it fails.
