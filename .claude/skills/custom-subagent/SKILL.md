---
name: custom-subagent
description: Create or improve a reusable Claude Code custom subagent by coordinating the custom-subagent worker and custom-advisor as parallel peers. Use manually when an agent definition is needed in a project or user-level .claude/agents directory.
argument-hint: "[custom subagent requirements]"
disable-model-invocation: true
---

Coordinate two named, same-level subagents for this request:

$ARGUMENTS

If no requirements were supplied, explain that `/custom-subagent` requires the desired role,
scope, capabilities, and constraints as its argument. Otherwise:

1. Start the `custom-advisor` type as a background subagent named `custom-advisor` with the same
   request. Ask it to assess requirements, risks, and design tradeoffs and to answer messages
   from the peer worker.
2. Immediately start the `custom-subagent` type as a separate background subagent named
   `custom-subagent` with the same request. Tell it that `custom-advisor` is its same-level peer
   and that it should communicate through `SendMessage` only when strategic advice adds value.
3. Keep both subagents at the same level under the main conversation. Do not have either one
   spawn the other.
4. Let the worker own inspection, implementation, and validation. Let the advisor remain
   read-only and advisory.
5. If direct peer messaging is unavailable, relay only the necessary messages through the main
   conversation without converting the relationship into nested delegation.
6. Wait for both results, resolve any disagreement using the available evidence, and report the
   completed changes and validation.
