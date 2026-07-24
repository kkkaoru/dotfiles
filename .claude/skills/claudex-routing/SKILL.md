---
name: claudex-routing
description: Route claudex work to config-defined provider agents by Codexbar capacity, select explicitly requested models dynamically, and consult the independent advisor when useful. Use automatically in the claudex orchestrator and manually when diagnosing or changing provider routing.
disable-model-invocation: true
---

# Claudex Routing

Use the routing context injected at prompt submission as the authoritative capacity snapshot for
the current turn. It contains only provider names, utilization percentages, routing fields, and
selected agents; account details from `codexbar` are never retained.

## Routing policy

1. Delegate substantive work primarily to agents in `selected_workers` with the available
   SubAgent tool (`Task` in current Claude Code, `Agent` in older versions). Pass their `model` and
   `effort` values as `claudex_model` and `claudex_effort`.
   When delegation is explicitly requested and the work is clear, invoke the selected SubAgent in
   the first response. Do not use task-list bookkeeping merely as a precondition for delegation.
2. If the user explicitly names a model that matches a provider's `model_prefixes`, select that
   provider dynamically and pass the exact requested model. The adapter resolves the matching
   backend lazily.
3. Use multiple selected workers for independent work or complementary review only when useful.
4. Use the configured fallback only when every capacity-managed provider is unavailable.
5. Invoke the configured `advisor` in addition to workers when explicitly requested or when a
   complex, ambiguous, high-risk, or consequential decision benefits from strategic review. The
   advisor never replaces an implementation worker and does not depend on provider quota.
6. Synthesize, verify, and present the subagents' results in the main conversation. Capacity
   selection does not relax repository instructions, safety requirements, or validation gates.
7. Count delegation as successful only after an actual SubAgent tool result. Never fabricate a
   selected worker response in the main session; report unavailable execution explicitly.

`scripts/route_usage.py` refreshes the capacity snapshot at most once every five minutes by
default. Set `CLAUDEX_USAGE_CACHE_SECONDS=0` to disable caching. A missing provider, unknown usage,
100% utilization in any reported quota window, malformed output, or a `codexbar` failure is
treated conservatively as unavailable.

After changing the routing script, run `uv run tests/run_coverage.py` from this skill directory.
The test runner measures statements and branches and fails below 95% coverage.

## Provider configuration

`.config/claudex/providers.json` is the shared source for the main provider, enabled providers,
default models, effort, model prefixes, capacity provider names, fallback, and advisor. The fish
launcher and routing hook both honor `CLAUDEX_PROVIDER_CONFIG` when a different file is needed.

To add a model for an existing provider, extend `modelPrefixes` or update `defaultModel`. To add an
ACP without a Rust change, add an enabled provider using `backend: "configured-acp"` and an `acp`
object:

```json
{
  "program": "new-provider",
  "arguments": ["--model", "{model}", "--acp", "--stdio"]
}
```

Arguments are passed directly without a shell, and every `{model}` occurrence is replaced with the
selected model. The provider's `agent` must name a Claude Code agent definition. Keep that agent's
frontmatter model set to `inherit`; claudex orchestration passes the config model and effort
explicitly. Pinning a provider model in the Agent definition can trigger Claude Code's native model
validation before the adapter receives the request.
