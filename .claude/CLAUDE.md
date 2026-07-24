# Orchestration

- Keep the main session focused on decomposition, coordination, synthesis, verification, and the
  final response. Delegating substantive investigation, implementation, and review to SubAgents is
  the standing default for every turn; the user does not need to repeat that preference.
- Use the available SubAgent tool (`Task` in current Claude Code, `Agent` in older versions) and
  the orchestration skill. In Claudex, follow `claudex-routing` and delegate primarily to its
  `selected_workers`, preserving each configured model and effort.
- Apply the current `selected_workers` routing to every Agent/Task launch, including launches from
  an already delegated worker. Nested delegation is allowed, but it must select the routed worker
  agent and pass that worker's exact `claudex_model` and `claudex_effort`; do not default a nested
  launch to generic `claude` or merely inherit the parent worker's route.
- When substantive work is clear and the user has not explicitly opted out of delegation, invoke
  the selected SubAgent directly in the first response. Do not merely announce future delegation.
  Do not add `TaskList`, `TaskCreate`, or `TaskUpdate` round trips solely to prepare delegation; use
  task tracking only when the work itself needs persistent dependency tracking.
- When the main session must await worker results before synthesis, launch independent Agent/Task
  calls together as foreground calls in one tool round. Use background execution only when useful
  independent work can continue or the delegated task must outlive the current turn. Background
  task notifications join the main session's next-turn input queue.
- Start as many SubAgents as useful for real parallelism or independent context. Before shutting
  down, abandoning, or replacing one, weigh likely follow-ups and potential prompt-prefix/cache
  reuse against slot and resource pressure. For a compatible follow-up, use `SendMessage` with the
  exact recipient specified by the prior Agent/Task result (agent ID or teammate name as applicable);
  never guess or persist recipients across sessions.
- Never copy the main session's model or effort into worker routing. If `selected_workers` is
  unavailable, report routing as unavailable instead of inventing a worker selection.
- Treat the current Claudex routing context as authoritative over stale auto-memory about worker or
  advisor model policy; do not inspect such memory before delegation.
- Use `custom-advisor` when requested or when a complex, ambiguous, high-risk, long-running, or
  stalled decision benefits from independent strategic review. The advisor advises; workers act.
- The main session owns decisions, resolves conflicts, and verifies delegated results. Agent/Task
  acceptance proves delegation; an actual worker reply or completion notification proves completion.
  A `SendMessage` delivery acknowledgement alone does not. Never fabricate a worker response or
  present main-session work as if it came from a worker. Interpret the TUI's `N queued` as pending input,
  which may include human prompts and background task notifications—not worker capacity, active
  slots, or `SendMessage` delivery. Handle work directly only when it is trivial, the user opts out,
  or delegation is unavailable; when unavailable, state that limitation.
