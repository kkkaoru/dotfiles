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
   The list is ordered by weekly remaining headroom descending (the `seven-day` quota window when
   a provider reports one, otherwise its aggregate usage), with the `five-hour` window breaking
   ties, then exact-model concurrency headroom; prefer `preferred_worker` for primary work. Treat
   an exact `model_concurrency` entry with `available: false` as unavailable for that turn.
   Automatic `selected_workers` already drops low weekly remaining peers (under ~25%) when any
   peer still has ample weekly headroom (at least ~40%); do not reintroduce those depleted models
   for diversity. Launch in ranking order: fill scopes from the front of `selected_workers` /
   `worker_ranking` before lower-ranked entries. Ollama API-only availability counts as full
   weekly headroom when CodexBar has no meter.
   A selected worker may intentionally use the same model as the outer session; outer and
   SubAgent requests are independent. The one conservation exception is the `claudex-sonnet`
   worker: when `CLAUDEX_OUTER_MODEL` is a Sonnet 5 alias, automatic routing omits that worker
   (including the empty-pool fallback) unless `CLAUDEX_ALLOW_SONNET_SUBAGENT=1` is an explicit
   policy opt-in. An explicit Agent/Task request with `claudex_model: claude-sonnet-5` remains
   valid unless the exact model is denylisted.
   When any selected worker exists, delegation is mandatory before substantive main-session work;
   direct execution is fallback-only when no worker is available or the user explicitly opts out.
   This includes WebSearch/WebFetch, repository reads, and implementation work.
   Claudex enforces the main-session side with both a UserPromptSubmit reminder and a
   `PreToolUse` hook (`claudex-tool-policy` Rust binary from `tools/claudex-tool-policy`,
   injected only into the claudex-isolated settings; plain `claude` is untouched). That hook
   denies Read/Write/Edit/Grep/Glob/Web tools in main while
   `delegation_required` is true (Bash stays allowed in main). SubagentStart reminders and
   PreToolUse both keep the worker's full tool set — main denials are never inherited. Parallel
   SubAgents still take exclusive file locks on Write/Edit targets; partition scopes so workers
   never share a mutable path.
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
   work or complementary review. Use multiple selected workers only when useful; start exactly
   `min(independent scopes, available worker slots, configured maximum)` ordinary workers. One
   indivisible scope means one worker, even when the pool is larger. Prefer distinct model kinds
   when the task already has two or more scopes and the pool provides them, but do not manufacture
   scopes or duplicate a launch to satisfy diversity. Report a genuine indivisible phase or
   capacity shortfall and re-evaluate fan-out at the next result, failure, capacity update, or phase
   boundary. `custom-advisor`
   is a separate logical session singleton/capacity channel, not one of these implementation
   workstreams; built-in `advisor()` remains independent of worker capacity.
   Apply this selection to every Agent/Task launch, including nested launches made by an existing
   worker. A nested launch must use the current selected worker's agent and exact model/effort; it
   must not default to generic `claude` or blindly inherit its parent's provider route.
   For several independent workers, treat unknown or potentially long-running work as asynchronous:
   emit each Agent/Task call as its own native background launch (`run_in_background: true`).
   Multiple launches may be emitted in the same assistant response, but never wrap them in an
   adapter-only batch or hold peers that already finished. Use foreground only for short, bounded work
   whose result is required before the next main action, or when the active user explicitly asks for
   synchronous completion. Do not use a foreground batch merely to gather all results. After a
   background launch, immediately start a concrete independent action or end the turn with a concise
   user-visible status. When completion notifications re-enter the next turn, integrate each available result
   without waiting for the slowest worker; never silently wait or keep hidden reasoning for pending
   notifications.
   Background workers are never fire-and-forget: record the exact task id from each launch result,
   but do not automatically call `TaskList`, poll on a timer, or issue `TaskOutput` for every task.
   Handle the user's next message first and retrieve only the exact task output needed by that
   message or an unresolved dependency. Use `TaskList` only for an explicit status request or when
   a dependency cannot be resolved from a completion event. If a task is still processing, preserve
   its task id and continue independent work; never retry or relaunch it solely because completion
   is delayed.
   Do not mix a long-running foreground worker into asynchronous background launches: it still
   holds the main session until its slowest foreground result returns. When an interactive permission
   really requires foreground execution, restrict it to that short permission-dependent operation
   and launch all other independent work as separate native background calls.
   Emit every intended independent Agent/Task call in the same assistant response and tool round,
   while keeping each call and lifecycle independent; never use an adapter-only batch wrapper.
   Never launch one and defer the rest. Do not announce a worker count unless that same response
   contains exactly that many launch calls.
4. Use the configured fallback only when every capacity-managed provider is unavailable. If that
   fallback is `claudex-sonnet` and the outer session already uses Sonnet 5, suppress automatic
   selection as described above; direct explicit Sonnet launches remain available.
5. Advisors are independent of worker capacity and never replace implementation workers:
   - Use Claude Code's built-in parameterless `advisor()` tool according to its standard policy. It
     automatically receives the complete conversation history and does not depend on provider quota.
   - Independently, invoke the `custom-advisor` SubAgent (`claude-fable-5` / `xhigh`) when explicitly
     requested, external research has multiple sources, a decision is complex/ambiguous or high-risk,
     a phase exceeds ten minutes, a worker fails/times out/stalls, or worker results conflict. Do not
     invoke it for trivial or deterministic tasks. Built-in `advisor()` and `custom-advisor` coexist;
     neither replaces the other.
     Its Agent/Task call must set `subagent_type: custom-advisor`, `claudex_model: claude-fable-5`,
     and `claudex_effort: xhigh`; generic-purpose is not an acceptable substitute. Verify
     `resolvedModel: claude-fable-5` in the completion result and treat any mismatch as routing
     failure rather than advisor success.
   - Account for `custom-advisor` separately from `selected_workers` and provider quota headroom. Do
     not spend worker slots on it and do not treat its presence as capacity pressure against workers.
   - Prefer one logical custom-advisor per session (reuse via native Agent/Task results and `TaskOutput`; not a hard process=1
     OS cap): resume the first compatible instance with the exact Agent/Task recipient, including
     after completion. Start another custom advisor only for true parallel or clean-room review,
     incompatible context, or an unavailable recipient. Workers should retrieve that advisor's result through the native task lifecycle
     when material guidance would change their work.
   - When `CLAUDEX_CUSTOM_ADVISOR` is `0`, `false`, or `off` (case-insensitive), skip only
     custom-advisor launches; built-in `advisor()` remains available. Unset or any other value leaves
     custom-advisor enabled.
6. Synthesize, verify, and present the subagents' results in the main conversation. Capacity
   selection does not relax repository instructions, safety requirements, or validation gates.
   For web research, preserve the evidence class for every material claim:
   - `fetch_verified` means the provider completed a fetch and returned the page content for the
     cited URL. It may support factual claims from that page.
   - `search_result_only` means the URL or claim appeared only in a native search result, title, or
     snippet. It is a discovery lead, not verification, and must not be cited as a confirmed source
     for names, dates, amounts, quotations, or other material facts.
   ACP providers can execute native WebSearch/WebFetch outside Claude Code's executable tool
   protocol. Therefore `tool_uses: 0`, or the absence of `tool_use`/`tool_result` blocks in a
   Claude transcript, is only a Claude-transcript observation; it does not prove that ACP-native
   evidence is absent. Inspect provider provenance before making that conclusion. Conversely, do
   not call a native search result a fetched page merely because the provider executed a tool.
   When the required `fetch_verified` evidence is unavailable, retry a permitted fetch or route the
   retrieval to a verified-capable worker. If that still fails, state the exact evidence limitation
   and that the fact or URL is unavailable; never fill the gap from memory, an unverified URL, or a
   worker's unsupported assertion.
7. Agent/Task acceptance proves delegation, not completion. Count delegated work as complete only
   after an actual worker reply or completion notification. Never fabricate a selected worker
   response; if execution is unavailable, continue safely in the main session and report it.
8. Treat worker and custom-advisor lifecycle as a deliberate decision:
   - For related follow-ups, reuse compatible workers through native Agent/Task results and `TaskOutput` instead of churning
     processes with fresh launches. Prefer a prior instance when the exact native Agent/Task recipient
     specified
     by its Agent/Task result (agent ID or teammate name as applicable) is available in the current
     main-session transcript and its agent, model, effort, role, scope, and authorization remain
     compatible with the current routing context. Send the smallest sufficient, self-contained
     delta, including new evidence the recipient has not seen, so the existing context and prompt
     prefix remain reusable.
   - Do not guess a recipient, persist it to memory, or call task-list tools solely to rediscover
     it. A delivery acknowledgement is not completion evidence; wait for the actual
     reply or completion notification. The TUI's `N queued` is pending main-session input, which may
     include human prompts and background task notifications—not worker capacity, active slots, or
     delivery. The latter reports its own worker-bound status separately.
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
     delta. Keep a stable scope key for every launch; never relaunch an in-flight, completed, or
     cancelled key. Do not refill a completed slot unless a genuinely unresolved scope with a new
     key already exists. Logical transcript reuse and live-process lifetime are separate: do not
     retain a live worker solely for possible reuse. During phases longer than ten minutes, perform
     a management tick every 600 seconds without imposing an active floor. On
     normal completion, cancellation, error, or main-session exit, the runtime must stop launches,
     request cancellation, wait for every owned child to exit, reap it, and then discard its session
     ownership record. The routing hook emits context only and cannot invoke Agent/Task/SendMessage;
     the main session owns those actions. Parallel controls: `CLAUDEX_SUBAGENT_MAX_PARALLEL` is the
     upper bound (never a required launch count). `CLAUDEX_SUBAGENT_MIN_PARALLEL` /
     `ACTIVE_FLOOR` / `MIN_MODEL_FAMILIES` are multi-scope phase targets when work can be
     decomposed; they never invent scopes for an indivisible single-scope task. Hook
     `orchestration.task_fanout_default` is the single-scope example only; use
     `task_fanout_examples` / `multi_scope_example_fanout` and
     `min(independent_scopes, max_available_workers, max_parallel_workers)` for real fan-out.
   - RAM-aware management is part of the same contract: every hook invocation samples macOS memory
     once and lowers `max_parallel_workers` when reclaimable RAM (free + inactive + speculative
     pages as a percent of total) is tight, and forces `reuse_compatible_workers` on at high or
     critical pressure. The memory cap only ever reduces the configured parallel upper bound and
     never raises the reuse or fan-out settings; a RAM-starved machine degrades to fewer, reused
     subagents instead of spawning until macOS kills applications. The per-invocation memory snapshot
     is independent of the five-minute usage cache because it must reflect live pressure.
     `memory_management` in the hook output carries `status`, `pressure_level`, `available_percent`,
     `configured_max_parallel_workers`, `effective_max_parallel_workers`, `management_active`, and
     `reuse_required`. Default bands (available percent of total RAM): below 10% cap 2, below 20%
     cap 6, below 30% cap 16, below 40% cap 32, otherwise no memory cap. Set
     `CLAUDEX_MEMORY_MANAGEMENT=0|false|off` to disable probing; the percentage bands are
     overridable with `CLAUDEX_MEMORY_AVAILABLE_PCT_CRITICAL|LOW|MEDIUM|MODERATE` and must stay
     ascending. On probe failure or non-macOS, status is `unavailable` and routing never blocks work
     on memory checks.

`tools/claudex-route-usage` (`claudex-route-usage`, typically installed to `~/.cargo/bin/claudex-route-usage`) refreshes the capacity snapshot at most once every five minutes by
default. Codex, Grok, Claude, Qwen Cloud (`qwencloud`), and other CodexBar-backed providers come
from `codexbar usage --json`. Primary/secondary windows are normalized to five-hour / seven-day
remaining for ranking.

Model selection is dynamic and codexbar-driven: `selected_workers` is ordered by weekly remaining
headroom descending, so the model with the most weekly capacity left is preferred for each
subagent launch. The weekly value is the explicit `seven-day` quota window when a provider
reports one, otherwise the provider's aggregate usage headroom; providers at 0% remaining are
excluded entirely. A reported `five-hour` window breaks ties between equally-utilized workers.
The hook output's `worker_capacity` list preserves that order and exposes each worker's
`used_percent`, `remaining_percent`, `weekly_remaining_percent`, and `five_hour_remaining_percent`
so the model choice is observably a runtime decision; unknown or unmetered usage reports `null`
for the window values and never outranks known headroom.

A SubAgent launched without an explicit `claudex_model` — most notably Claude Code's built-in
`general-purpose` type — would otherwise bypass this ranking and reach the adapter with
`native_model=None`, which has no recoverable route. The hook output's `default_subagent_route`
names the top-ranked worker (`selected_workers[0]`, the model with the most weekly capacity) so
those launches resolve to the dynamic winner instead of being excluded from selection. It carries
`agent`, `model`, `effort`, `applies_to_subagent_types: ["general-purpose"]`, and
`applies_when_claudex_model_omitted: true`, and is `null` only when no worker is selectable at all.

The routing context is loaded into subagent sessions as well. Claude Code injects
`UserPromptSubmit` `additionalContext` only into the main session, so the same binary is also
registered on `SubagentStart` (`claudex-route-usage --event SubagentStart`): Claude Code places that
event's `additionalContext` at the start of the subagent conversation, giving every routed worker
the identical sanitized context — `selected_workers`, `disabled_subagent_models`, memory policy,
and `worker_capacity` — for its own nested Agent/Task launches. The 5-minute usage cache keeps the
per-spawn cost small, and the daemon denylist remains the authoritative hard enforcement at the
API boundary regardless of what a worker sees.

If the browser session has expired or quota refresh otherwise fails, use Qwen Code's existing
compatible API configuration to make a non-generative `GET /models` availability check. A
successful check keeps Qwen available with unknown headroom, after providers with known remaining
capacity. A failed check disables only Qwen. A Codexbar failure likewise disables only its
providers. Qwen with known quota participates in the same weekly-remaining-first ordering as Codex
and Grok, with its five-hour window breaking weekly ties. Set `CLAUDEX_USAGE_CACHE_SECONDS=0` to
disable the five-minute routing-summary cache; the
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

Define persistent exact model IDs in a gitignored per-machine denylist:

- `~/.config/claudex/disabled-subagent-models.$(hostname -s).local.json` (preferred)
- `~/.config/claudex/disabled-subagent-models.local.json`
- tracked empty baseline: `.config/claudex/disabled-subagent-models.json`

Set `CLAUDEX_DISABLED_SUBAGENT_MODELS_CONFIG` before starting `claudex` to select a different
dedicated file for one terminal, and use comma-separated `CLAUDEX_DISABLED_SUBAGENT_MODELS` for
additional terminal-only entries. These settings do not disable the outer main-session model. The
merged policy participates in the routing cache key and is enforced again by the adapter before
provider execution, so a stale prompt or an explicit Agent field cannot bypass it.

After changing the routing crate, run `cargo test` from `tools/claudex-route-usage`.
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
