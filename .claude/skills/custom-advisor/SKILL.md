---
name: custom-advisor
description: Request independent strategic guidance from the custom-advisor subagent for complex, ambiguous, or long-running work. Use manually before implementation, at an important decision point, or when progress has stalled.
argument-hint: "[task, decision, or problem to advise on]"
context: fork
agent: custom-advisor
disable-model-invocation: true
---

Provide strategic advice for the following request:

$ARGUMENTS

Identify the objective and constraints, challenge assumptions, compare viable approaches,
recommend a direction, and give concrete next actions and verification criteria. Do not execute
the task. If no request was supplied, explain that `/custom-advisor` requires a task, decision,
or problem as its argument.
