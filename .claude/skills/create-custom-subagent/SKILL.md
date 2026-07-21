---
name: create-custom-subagent
description: Create or improve a reusable Claude Code custom subagent through the custom-subagent-creator agent. Use manually when an agent definition is needed in a project or user-level .claude/agents directory.
argument-hint: "[custom subagent requirements]"
context: fork
agent: custom-subagent-creator
disable-model-invocation: true
---

Create or update a Claude Code custom subagent that satisfies these requirements:

$ARGUMENTS

Inspect the applicable repository instructions and existing definitions, use the requested
scope, and follow the current official Claude Code subagent specification. Preserve unrelated
changes, validate the completed definition, and summarize the resulting path and important
configuration. If no requirements were supplied, explain that `/create-custom-subagent`
requires the desired role, scope, capabilities, and constraints as its argument.
