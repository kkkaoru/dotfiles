---
name: custom-subagent
description: Create or improve a reusable Claude Code custom subagent with the custom-subagent worker and Claude Code's built-in advisor. Use manually when an agent definition is needed in a project or user-level .claude/agents directory.
argument-hint: "[custom subagent requirements]"
disable-model-invocation: true
---

Coordinate the built-in advisor and the custom-subagent worker for this request:

$ARGUMENTS

If no requirements were supplied, explain that `/custom-subagent` requires the desired role,
scope, capabilities, and constraints as its argument. Otherwise:

1. Call the built-in parameterless `advisor()` tool before committing to the agent design. It sees
   the complete conversation history automatically, so do not create or launch a custom advisor.
2. Start the `custom-subagent` type with the request and the material advisor guidance.
3. Let the worker own inspection, implementation, and validation with the main session's complete
   tool set and permission context. Keep the built-in advisor advisory rather than delegating
   implementation to it.
4. Reconcile the worker result with the advisor guidance using the available evidence, then report
   the completed changes and validation.
