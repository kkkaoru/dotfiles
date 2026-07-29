---
name: custom-subagent
description: Create or improve a reusable Claude Code custom subagent by coordinating the custom-subagent worker with independent advisory channels. Use manually when an agent definition is needed in a project or user-level .claude/agents directory.
argument-hint: "[custom subagent requirements]"
disable-model-invocation: true
---

Coordinate advisory review and the custom-subagent worker for this request:

$ARGUMENTS

If no requirements were supplied, explain that `/custom-subagent` requires the desired role,
scope, capabilities, and constraints as its argument. Otherwise:

1. Call the built-in parameterless `advisor()` tool when its standard policy applies. It sees the
   complete conversation history automatically and remains independent of the custom-advisor SubAgent.
2. Unless `CLAUDEX_CUSTOM_ADVISOR` is `0`, `false`, or `off` (case-insensitive), start or reuse the
   `custom-advisor` type (`claude-fable-5` / `xhigh`) as a peer SubAgent for strategic requirements,
   risks, and design tradeoffs. Prefer the first compatible session custom advisor and continue it
   with `SendMessage` using the exact prior Agent/Task recipient, including after completion. Start
   another custom advisor only for true parallel or clean-room review, incompatible context, or an
   unavailable recipient. Do not count custom-advisor against worker capacity.
3. Start the `custom-subagent` type with the request and material advisory guidance. Tell it that
   `custom-advisor` is a same-level peer when one is active, and that peer `SendMessage` is for
   strategic advice that would change the work—not routine status chatter.
4. Keep both the worker and custom-advisor at the same level under the main conversation. Do not
   have either one spawn the other. Let the worker own inspection, implementation, and validation
   with the main session's complete tool set and permission context. Keep both advisors advisory
   rather than delegating implementation to them.
5. If direct peer messaging is unavailable, relay only the necessary messages through the main
   conversation without converting the relationship into nested delegation.
6. Reconcile the worker result with built-in advisor and custom-advisor guidance using the available
   evidence, then report the completed changes and validation.
