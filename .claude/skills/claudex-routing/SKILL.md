---
name: claudex-routing
description: Route claudex work to config-defined provider agents by Codexbar or provider-CLI usage, select explicitly requested models dynamically, and keep built-in advisor() plus custom-advisor independent of worker capacity. Use automatically in the claudex orchestrator and manually when diagnosing or changing provider routing.
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
   The list is ordered by the tightest known quota or exact-model concurrency headroom; prefer
   `preferred_worker` for primary work. Treat an exact `model_concurrency` entry with
   `available: false` as unavailable for that turn.
   A selected worker may intentionally use the same model as the outer session; outer and
   SubAgent requests are independent, so model identity alone never makes a worker unavailable.
   Pass each worker's `model` and `effort` as `claudex_model` and `claudex_effort`. When substantive
   work is clear, invoke the selected SubAgent in the first response rather than merely announcing
   future delegation. Do not use task-list bookkeeping merely as a precondition for delegation.
2. If the user explicitly names a model that matches a provider's `model_prefixes`, select that
   provider dynamically and pass the exact requested model only when it is not listed in
   `disabled_subagent_models` and its exact `model_concurrency` entry is not unavailable. A missing
   dynamic entry inherits the provider's `max_concurrency` and means the daemon observed no active
   request for that exact model. The denylist, merged from the dedicated config and terminal overrides,
   is absolute and takes precedence over explicit requests, inheritance, prior
   recipients, and capacity. The adapter resolves an allowed matching backend lazily. If no allowed
   worker remains, continue in the main session and report that SubAgent routing is unavailable.
3. The main session must control parallel distribution across multiple SubAgents for independent
   work or complementary review. Use multiple selected workers only when useful; start the number
   needed for genuine parallelism, role separation, and clean independent context; reuse policy
   must not suppress useful fan-out.
   Before launching a substantive, non-trivial phase, explicitly decompose it into non-redundant
   workstreams and select the fan-out dynamically for task content and current capacity. Launch at
   least three ordinary workers in the same background batch whenever the phase is divisible and
   capacity permits; if fewer than three natural workstreams exist, use implementation, independent
   verification, and risk/review roles rather than silently serializing. Use at least two distinct
   model kinds whenever allowed workers provide them. Report a genuine indivisible phase or
   capacity shortfall and re-evaluate fan-out at the next result, failure, capacity update, or phase
   boundary. Avoid serial heavy processing by one worker: do not give an entire heavy
   or unknown-duration task to one ordinary worker merely because it is convenient. `custom-advisor`
   is a separate logical session singleton/capacity channel, not one of these implementation
   workstreams; built-in `advisor()` remains independent of worker capacity.
   Apply this selection to every Agent/Task launch, including nested launches made by an existing
   worker. A nested launch must use the current selected worker's agent and exact model/effort; it
   must not default to generic `claude` or blindly inherit its parent's provider route.
   When the user asks for findings, an answer, or completed work in the current reply, every required worker result is a dependency: launch the entire batch with `run_in_background: false`, wait for actual replies, and synthesize them before responding. Use background launches only when the user explicitly asks for asynchronous progress or the current response does not require the result. After a
   background launch, immediately start a concrete independent action or end the turn with a concise
   user-visible status. When completion notifications re-enter the next turn, integrate each available result
   without waiting for the slowest worker; never silently wait or keep hidden reasoning for pending
   notifications.
   Do not mix a long-running foreground worker into a background worker batch: it still holds the
   main session until its slowest foreground result returns. When an interactive permission really
   requires foreground execution, restrict it to that short permission-dependent operation and
   launch all other independent work separately in the background.
   Emit every intended independent Agent/Task call in the same assistant response and tool round;
   never launch one and defer the rest. Do not announce a worker count unless that same response
   contains exactly that many launch calls.
4. Use the configured fallback only when every capacity-managed provider is unavailable.
5. Advisors are independent of worker capacity and never replace implementation workers:
   - Use Claude Code's built-in parameterless `advisor()` tool according to its standard policy. It
     automatically receives the complete conversation history and does not depend on provider quota.
   - Independently, invoke the `custom-advisor` SubAgent (`claude-fable-5` / `xhigh`) when explicitly
     requested or when a complex, ambiguous, high-risk, long-running, or stalled decision benefits
     from strategic review that can message peers. Built-in `advisor()` and `custom-advisor`
     coexist; neither replaces the other.
   - Account for `custom-advisor` separately from `selected_workers` and provider quota headroom. Do
     not spend worker slots on it and do not treat its presence as capacity pressure against workers.
   - Prefer one logical custom-advisor per session (reuse via `SendMessage`; not a hard process=1
     OS cap): resume the first compatible instance with the exact Agent/Task recipient, including
     after completion. Start another custom advisor only for true parallel or clean-room review,
     incompatible context, or an unavailable recipient. Workers may `SendMessage` that same advisor
     when material guidance would change their work.
   - When `CLAUDEX_CUSTOM_ADVISOR` is `0`, `false`, or `off` (case-insensitive), skip only
     custom-advisor launches; built-in `advisor()` remains available. Unset or any other value leaves
     custom-advisor enabled.
6. Synthesize, verify, and present the subagents' results in the main conversation. Capacity
   selection does not relax repository instructions, safety requirements, or validation gates.
7. Agent/Task acceptance proves delegation, not completion. Count delegated work as complete only
   after an actual worker reply or completion notification. Never fabricate a selected worker
   response; if execution is unavailable, continue safely in the main session and report it.
8. Treat worker and custom-advisor lifecycle as a deliberate decision:
   - For related follow-ups, reuse compatible workers with `SendMessage` instead of churning
     processes with fresh launches. Prefer a prior instance when the exact `SendMessage` recipient
     specified
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
     Do not send a mid-flight message merely to repeat scope or restrictions already present in the
     original delegation. A message queued to a busy worker does not add parallel capacity; assign
     genuinely independent work to another routed worker when useful capacity exists.
   - Start a new instance when true concurrency, a clean-room review, an independent second opinion,
     a different route/model/effort/role, incompatible scope or authorization, or an unavailable
     recipient requires it. For custom-advisor, prefer the logical session singleton and only fan
     out under those exceptional conditions.
   - Before explicitly shutting down or discarding an instance, weigh likely follow-ups and
     potential prompt-prefix/cache reuse against slot pressure, resource cost, stale or contaminated
     context, and whether the role is genuinely complete. Termination is allowed when those factors
     favor it; it is not automatic merely because one delegated task ended. A completed agent may be
     logically resumable without a live process, so do not keep it artificially busy. Apply the same
     deliberate reuse rule to custom-advisor, counted separately from worker slots.
   - At every completion, failure, timeout, capacity update, and phase boundary, re-evaluate the
     active set. Integrate partial results immediately, reuse a compatible recipient for a related
     delta, and fill newly available capacity only with genuinely independent unresolved work or
     review risk. Logical transcript reuse and live-process lifetime are separate: do not retain a
     live worker solely for possible reuse. After each worker completion, decide whether to stop a
     stale worker, send concrete additional instructions to an active worker, reuse a compatible
     recipient for the same content, or launch a new selected worker for the same or supplemental
     content. During phases longer than ten minutes, perform a management tick every 600 seconds;
     when ordinary active workers fall to one, interrupt/cancel the stale sole worker as appropriate
     and add, reuse, or message work until at least two remain active whenever capacity permits. On
     normal completion, cancellation, error, or main-session exit, the runtime must stop launches,
     request cancellation, wait for every owned child to exit, reap it, and then discard its session
     ownership record. The routing hook emits context only and cannot invoke Agent/Task/SendMessage;
     the main session owns those actions. Its validated controls are
     `CLAUDEX_SUBAGENT_MIN_PARALLEL`, `CLAUDEX_SUBAGENT_ACTIVE_FLOOR`,
     `CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION`, `CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS`,
     `CLAUDEX_SUBAGENT_MIN_MODEL_FAMILIES`, `CLAUDEX_SUBAGENT_REUSE`, and
     `CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT`; no hard maximum process cap is imposed.

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

The hook reads the shared daemon's loopback `/health` on every prompt, independently of the usage
cache. Providers with `maxConcurrency` inherit that positive exact-model limit for every dynamic
route selected through `modelPrefixes`. `model_concurrency` exposes only sanitized model IDs and
`active`, `queued`, `limit`, and `available` state. A full exact model is excluded. The health URL
comes from explicit `CLAUDEX_DAEMON_HEALTH_URL`, a loopback `ANTHROPIC_BASE_URL` origin, or the
default `127.0.0.1:8318` daemon, in that order. If health is temporarily
unavailable, the worker remains launchable with unknown slot headroom because the adapter enforces
the hard limit authoritatively.

Define persistent exact model IDs in `.config/claudex/disabled-subagent-models.json`. Set
`CLAUDEX_DISABLED_SUBAGENT_MODELS_CONFIG` before starting `claudex` to select a different dedicated
file for one terminal, and use comma-separated `CLAUDEX_DISABLED_SUBAGENT_MODELS` for additional
terminal-only entries. These settings do not disable the outer main-session model. The merged policy
participates in the routing cache key and is enforced again by the adapter before provider execution,
so a stale prompt or an explicit Agent field cannot bypass it.

After changing the routing script, run `uv run tests/run_coverage.py` from this skill directory.
The test runner measures statements and branches and fails below 95% coverage.
For orchestration changes, also exercise the acceptance contract: same-round background fan-out for
a heavy phase, compatible-recipient reuse, partial-result integration without waiting for the
slowest worker, and normal/cancel/session-exit child reaping in the runtime integration tests.

## Provider configuration

`.config/claudex/providers.json` is the shared source for the main provider, enabled providers,
default models, effort, model prefixes, capacity provider names, and fallback. Worker capacity
selection does not include `custom-advisor`; that SubAgent is defined under `.claude/agents/` and
orchestrated independently of provider quota. The fish launcher and routing hook both honor
`CLAUDEX_PROVIDER_CONFIG` when a different file is needed.

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
