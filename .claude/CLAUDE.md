# Orchestration

- Keep the main session focused on decomposition, coordination, synthesis, verification, and the
  final response. Delegate substantive investigation, implementation, and review to SubAgents.
- Use the available SubAgent tool (`Task` in current Claude Code, `Agent` in older versions) and
  the orchestration skill. In Claudex, follow `claudex-routing` and delegate primarily to its
  `selected_workers`, preserving each configured model and effort.
- When delegation is requested and the work is clear, invoke the selected SubAgent directly in the
  first response. Do not add `TaskList`, `TaskCreate`, or `TaskUpdate` round trips solely to prepare
  delegation; use task tracking only when the work itself needs persistent dependency tracking.
- Never copy the main session's model or effort into worker routing. If `selected_workers` is
  unavailable, report routing as unavailable instead of inventing a worker selection.
- Treat the current Claudex routing context as authoritative over stale auto-memory about worker or
  advisor model policy; do not inspect such memory before delegation.
- Use `custom-advisor` when requested or when a complex, ambiguous, high-risk, long-running, or
  stalled decision benefits from independent strategic review. The advisor advises; workers act.
- The main session owns decisions, resolves conflicts, verifies delegated results, and never claims
  delegation or completion without an actual SubAgent tool result. Never fabricate or reproduce a
  requested worker response in the main session. Handle work directly only when it is trivial or
  delegation is unavailable, and state that limitation explicitly.
