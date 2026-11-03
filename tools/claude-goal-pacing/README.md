# claude-goal-pacing

A Claude Code mod that stops `/goal` from re-checking the same unchanged state over and over.

`/goal` checks after every turn and continues while the goal is unmet. When the goal needs the
person's judgment, the model has nothing to do, answers "waiting for you", and the check sends it
back at once, repeating until Claude Code's cap of 9 blocked stops.

This mod lets a goal continuation end the turn when it used no tool since the previous check,
without running the check again, and shows a toast. The goal stays set: the next message continues
the work and the check runs again. Continuations that used a tool are checked as before, and
subagent tool calls do not count as work of the main turn.

Loaded in every session through `CLAUDE_CODE_PLUGIN_DIRS` in `.claude/settings.json`. Check with
`bun install && bun run check`.
