---
name: claudex-routing
description: Route claudex work to config-defined provider agents by Codexbar or provider-CLI usage and select explicitly requested models dynamically. Use automatically in the claudex orchestrator and manually when diagnosing or changing provider routing.
disable-model-invocation: true
---

# Claudex Routing

Use the routing context injected at prompt submission as the authoritative capacity snapshot for
the current turn. It contains only provider names, sanitized utilization, routing fields, and
selected agents; account details, browser credentials, API keys, and raw provider output are never
retained.

## Routing policy

1. By default, delegate substantive work primarily to agents in `selected_workers` with the
   available SubAgent tool (`Task` in current Claude Code, `Agent` in older versions), unless the
   user explicitly opts out. This is the standing default; do not wait for the user to repeat it.
   Every worker inherits the main session's complete tool set and permission context. Never add an
   implicit read-only, plan-only, no-edit, no-build, or no-deploy restriction to the definition or
   delegation prompt. Use foreground execution when background execution would auto-deny an
   interactive permission available to the main session.
   The list is ordered by known quota headroom; prefer `preferred_worker` for primary work.
   Pass each worker's `model` and `effort` as `claudex_model` and `claudex_effort`. When substantive
   work is clear, invoke the selected SubAgent in the first response rather than merely announcing
   future delegation. Do not use task-list bookkeeping merely as a precondition for delegation.
2. If the user explicitly names a model that matches a provider's `model_prefixes`, select that
   provider dynamically and pass the exact requested model only when it is not listed in
   `disabled_subagent_models`. That list, merged from the dedicated config and terminal overrides,
   is an absolute SubAgent denylist and takes precedence over explicit requests, inheritance, prior
   recipients, and capacity. The adapter resolves an allowed matching backend lazily. If no allowed
   worker remains, continue in the main session and report that SubAgent routing is unavailable.
3. Use multiple selected workers for independent work or complementary review only when useful.
   Start the number needed for genuine parallelism, role separation, and clean independent context;
   reuse policy must not suppress useful fan-out.
   Apply this selection to every Agent/Task launch, including nested launches made by an existing
   worker. A nested launch must use the current selected worker's agent and exact model/effort; it
   must not default to generic `claude` or blindly inherit its parent's provider route.
   When the main session must await results before synthesis, launch independent calls together as
   foreground calls in one tool round. Use background execution only when useful independent work
   can continue or the task must outlive the turn; its notifications join the next-turn input queue.
4. Use the configured fallback only when every capacity-managed provider is unavailable.
5. Use Claude Code's built-in parameterless `advisor()` tool according to its standard policy. It
   automatically receives the complete conversation history, never replaces an implementation
   worker, and does not depend on provider quota. Do not launch or model-route a custom advisor.
6. Synthesize, verify, and present the subagents' results in the main conversation. Capacity
   selection does not relax repository instructions, safety requirements, or validation gates.
7. Agent/Task acceptance proves delegation, not completion. Count delegated work as complete only
   after an actual worker reply or completion notification. Never fabricate a selected worker
   response; if execution is unavailable, continue safely in the main session and report it.
8. Treat worker lifecycle as a deliberate decision:
   - Reuse a prior instance for a related follow-up when the exact `SendMessage` recipient specified
     by its Agent/Task result (agent ID or teammate name as applicable) is available in the current
     main-session transcript and its agent, model, effort, role, scope, and authorization remain
     compatible with the current routing context. Send the smallest sufficient, self-contained
     delta, including new evidence the recipient has not seen, so the existing context and prompt
     prefix remain reusable.
   - Do not guess a recipient, persist it to memory, or call task-list tools solely to rediscover
     it. A `SendMessage` delivery acknowledgement is not completion evidence; wait for the actual
     reply or completion notification. The TUI's `N queued` is pending main-session input, which may
     include human prompts and background task notifications—not worker capacity, active slots, or
     `SendMessage` delivery. The latter reports its own worker-bound delivery status separately.
   - Start a new instance when true concurrency, a clean-room review, an independent second opinion,
     a different route/model/effort/role, incompatible scope or authorization, or an unavailable
     recipient requires it.
   - Before explicitly shutting down or discarding an instance, weigh likely follow-ups and
     potential prompt-prefix/cache reuse against slot pressure, resource cost, stale or contaminated
     context, and whether the role is genuinely complete. Termination is allowed when those factors
     favor it; it is not automatic merely because one delegated task ended. A completed agent may be
     logically resumable without a live process, so do not keep it artificially busy.

`scripts/route_usage.py` refreshes the capacity snapshot at most once every five minutes by
default. Codex and Grok usage comes from `codexbar usage --json`. Qwen's five-hour and seven-day
Token Plan utilization comes from the validated Qwen Cloud request saved in `tmp/curl.txt`. Only
sanitized percentages, reset times, and the acquisition time as UTC ISO 8601 `fetched_at` are saved
to `~/.cache/claudex/qwen-quota.json` with mode `0600`. Each read parses that stored acquisition
time; quota acquired less than one hour ago is reused, and at exactly one hour it is refreshed.

If the browser session has expired or quota refresh otherwise fails, use Qwen Code's existing
compatible API configuration to make a non-generative `GET /models` availability check. A
successful check keeps Qwen available with unknown headroom, after providers with known remaining
capacity. A failed check disables only Qwen. A Codexbar failure likewise disables only its
providers. Qwen with known quota participates in the same lowest-used-percentage ordering as Codex
and Grok. Set `CLAUDEX_USAGE_CACHE_SECONDS=0` to disable the five-minute routing-summary cache; the
one-hour Qwen quota cache remains independent. Missing, unknown, malformed, exhausted, or failed
usage is treated conservatively for the affected provider.

Define persistent exact model IDs in `.config/claudex/disabled-subagent-models.json`. Set
`CLAUDEX_DISABLED_SUBAGENT_MODELS_CONFIG` before starting `claudex` to select a different dedicated
file for one terminal, and use comma-separated `CLAUDEX_DISABLED_SUBAGENT_MODELS` for additional
terminal-only entries. These settings do not disable the outer main-session model. The merged policy
participates in the routing cache key and is enforced again by the adapter before provider execution,
so a stale prompt or an explicit Agent field cannot bypass it.

After changing the routing script, run `uv run tests/run_coverage.py` from this skill directory.
The test runner measures statements and branches and fails below 95% coverage.

## Provider configuration

`.config/claudex/providers.json` is the shared source for the main provider, enabled providers,
default models, effort, model prefixes, capacity provider names, and fallback. The fish
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
selected model. The provider's `agent` must name a Claude Code agent definition whose fixed
frontmatter model matches `defaultModel`; claudex orchestration also passes that model and effort
explicitly. Verify a new external model against the installed Claude Code because native validation
can reject an unsupported ID before the adapter receives the request.
