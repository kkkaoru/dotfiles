---
name: claudex-routing
description: Route claudex work to configured GPT, Grok, or Sonnet subagents from the current Codexbar quota summary. Use automatically in the claudex orchestrator and manually when diagnosing or changing provider-capacity routing.
disable-model-invocation: true
---

# Claudex Routing

Use the routing context injected at prompt submission as the authoritative capacity snapshot for
the current turn. It contains only provider names, utilization percentages, and selected agent
names; account details from `codexbar` are never retained.

## Routing policy

1. Delegate substantive work primarily to every agent listed in `selected_agents`.
2. When both `claudex-gpt` and `claudex-grok` are selected, split independent work between them
   or request complementary reviews when that improves the result. Parallelism is optional for
   small or sequential tasks.
3. When only one provider agent is selected, use that agent for the primary delegated work.
4. Use `claudex-sonnet` only when both external providers are unavailable, or when the injected
   context explicitly selects it.
5. Synthesize, verify, and present the subagents' results in the main conversation. Capacity
   selection does not relax repository instructions, safety requirements, or validation gates.

`scripts/route_usage.py` refreshes the capacity snapshot at most once every five minutes by
default. Set `CLAUDEX_USAGE_CACHE_SECONDS=0` to disable caching. A missing provider, unknown usage,
100% utilization in any reported quota window, malformed output, or a `codexbar` failure is
treated conservatively as unavailable.

## Manual model updates

Provider model IDs and effort levels are intentionally stored in agent frontmatter rather than
in the adapter implementation:

- `.claude/agents/claudex-gpt.md`
- `.claude/agents/claudex-grok.md`
- `.claude/agents/claudex-sonnet.md`

Update those files when provider model names change. The adapter routes unconfigured `gpt*` and
`grok*` model IDs lazily, so no Rust change is required for a normal model-version update.
