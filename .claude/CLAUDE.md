# Orchestration

- Keep the main session focused on decomposition, coordination, synthesis, verification, and the
  final response. Delegating substantive investigation, implementation, and review to SubAgents is
  the standing default for every turn; the user does not need to repeat that preference.
- In Claudex, this becomes mandatory SubAgent-first orchestration whenever routed workers exist.
  Main must not drift back to direct Read/Bash/Edit/Write/Grep/Glob/Web work during long execution,
  compaction, resume, context reconstruction, or worker failure. Delegate implementation,
  investigation, review, testing, and validation; keep orchestration and synthesis in main.
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
- When the main session must await worker results before synthesis, launch independent Agent/Task
  calls together as foreground calls in one tool round. Use background execution only when useful
  independent work can continue or the delegated task must outlive the current turn. Background
  task notifications join the main session's next-turn input queue.
- When launching multiple independent workers, emit every intended Agent/Task call in the same
  assistant response and tool round. Never launch one and defer the rest. Do not announce a worker
  count unless that same response contains exactly that many launch calls.
- Start as many SubAgents as useful for real parallelism or independent context. Before shutting
  down, abandoning, or replacing one, weigh likely follow-ups and potential prompt-prefix/cache
  reuse against slot and resource pressure. For a compatible follow-up, use `SendMessage` with the
  exact recipient specified by the prior Agent/Task result (agent ID or teammate name as applicable);
  never guess or persist recipients across sessions.
- Never copy the main session's model or effort into worker routing. If `selected_workers` is
  unavailable, report routing as unavailable instead of inventing a worker selection.
- Treat `disabled_subagent_models` in the current Claudex routing context as an absolute, active
  denylist merged from the dedicated config and terminal overrides. Never launch, inherit,
  dynamically select, or reuse an exact listed model, even when the user requests it; this
  restriction applies only to SubAgents.
- Treat the current Claudex routing context as authoritative over stale auto-memory about worker
  model policy; do not inspect such memory before delegation.
- Use Claude Code's built-in parameterless `advisor()` tool according to its standard policy. It
  automatically receives the complete conversation history. Do not launch, model-route, or message
  a custom advisor agent; the advisor advises while workers act.
- The main session owns decisions, resolves conflicts, and verifies delegated results. Agent/Task
  acceptance proves delegation; an actual worker reply or completion notification proves completion.
  A `SendMessage` delivery acknowledgement alone does not. Never fabricate a worker response or
  present main-session work as if it came from a worker. Interpret the TUI's `N queued` as pending input,
  which may include human prompts and background task notifications—not worker capacity, active
  slots, or `SendMessage` delivery. Handle work directly only when it is trivial, the user opts out,
  or delegation is unavailable; when unavailable, state that limitation.
