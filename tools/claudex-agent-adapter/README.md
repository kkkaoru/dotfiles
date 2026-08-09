# claudex agent adapter

For end-user setup, cross-Mac installation, routing configuration, and daily usage, see the
[Claudex guide](../../.config/claudex/README.md).

This local Rust service presents the subset of Anthropic's Messages API used by
Claude Code and routes it to built-in or config-defined agent backends:

| `--backend-route MODEL=BACKEND` | Backend protocol | Tool runtime |
| --- | --- | --- |
| `codex-app-server` | Codex app-server JSON-RPC | Claude Code tools bridged through Codex |
| `configured-acp` | Configured Agent Client Protocol command | Provider-owned agent tools and permission requests |
| `copilot-acp` | GitHub Copilot CLI Agent Client Protocol (ACP) | Copilot CLI agent tools and permission requests |
| `grok-acp` | Grok Build Agent Client Protocol (ACP) | Grok Build agent tools and permission requests |

All routes coexist in one daemon without eagerly starting provider processes.
Each configured backend starts lazily on its first model request and remains
available for reuse for the daemon's lifetime. Every request, including the main
Claude Code session, is routed from its actual Messages API `model` value. Native
Claude and genuinely unconfigured models retain the Claude subscription subprocess
behavior; a configured external-provider model uses its exact route. If that declared
provider cannot start, the request errors instead of being remapped to another model.

The Codex backend keeps threads alive while Claude Code executes dynamic tool
calls, then sends Claude Code's `tool_result` blocks back to the pending
app-server request. The Copilot backend launches
`copilot --acp --stdio --model MODEL`; Copilot is a backend choice rather than
a separate model family, so an explicit route such as
`--backend-route MODEL=copilot-acp` sends that model through the authenticated
GitHub Copilot CLI. The configured Grok route launches
`grok --model grok-4.5 --reasoning-effort high agent --always-approve stdio`,
so its `high` effort is launch-scoped rather than prompt metadata. It
creates ACP sessions, streams agent message chunks, and selects `AllowOnce` when
either ACP agent requests permission for a tool. The selected ACP provider owns
execution of its tools; Claude Code remains the outer conversation UI. Independent
ACP sessions, including parallel SubAgents, progress concurrently over the shared
provider connection. All ACP backends that share this bridge (Grok ACP and Copilot ACP) map protocol
updates into Claude Code surfaces:

| ACP | Claude Code |
| --- | --- |
| `AgentThoughtChunk` | thinking panel |
| `AgentMessageChunk` | assistant text |
| `ToolCall` / `ToolCallUpdate` | ephemeral WIP progress (live only; not answer text) |
| `Plan` | compact `Plan done/total` status (debounced; not a full checklist dump) |
| Session mode / title | ignored (noisier than Claude Code's own session UI) |
| xAI SubAgent / retry extensions | short ephemeral status |

Provider-owned tools are never emitted as executable Anthropic `tool_use` blocks,
so Claude Code cannot re-execute them or send synthetic missing-tool results.
Progress is streamed for live visibility, then stripped from the committed
assistant message/transcript so history stays answer-focused like native Claude
Code turns. Grok-native and Copilot-native nested work stays inside its ACP
provider session. Adapter-routed cross-provider Agent/Task launches still require
an explicit `claudex_model`; main orchestration receives their results and performs
the cross-provider integration.

Streaming requests return their HTTP response immediately. Each Codex
`item/agentMessage/delta` notification is converted to an Anthropic
`content_block_delta` SSE event instead of being buffered until turn completion.
Subscription subprocesses likewise use Claude Code's `stream-json` output and
forward text deltas as they arrive. Streaming responses open immediately with
`message_start` so Anthropic `ping` SSE events keep Claude Code's ~180s raw-byte
idle watchdog alive while the provider session is still being prepared. Activity
heartbeats are silence-only: after about 30s without visible provider output the
adapter emits a waiting status, then zero-width heartbeats every 30s. Real text,
thinking, or tool progress resets that timer. Heartbeats never accumulate into
the final answer text.

For `codex-app-server`, the adapter starts `codex app-server` with an isolated
`CODEX_HOME`. Codex authentication and only the user's `model_providers`
configuration are copied into that home; Claude Code
remains responsible for tools, hooks, MCP servers, skills, approvals, and
project instructions. The child inherits non-empty provider credentials from
the daemon environment. For a missing `env_key` declared by copied
`model_providers` configuration, the adapter reads only that variable from the
source `CODEX_HOME/.env`, then the user home `.env`; unrelated dotenv values are
not forwarded and credential values are not logged. Restart the shared daemon
after changing these credentials because the persistent app-server child receives
them only when it starts. `CLAUDEX_CODEX_PROGRAM`, `CLAUDEX_COPILOT_PROGRAM`,
`CLAUDEX_GROK_PROGRAM`, and `CLAUDEX_CLAUDE_PROGRAM` are development-only
executable overrides used by process integration tests.

Build and install with Rust 1.97.1:

The crate's Cargo config keeps direct debug/test artifacts outside the
checkout. For normal builds and installs, use the repository's ephemeral Cargo
wrapper: it allocates a unique temporary target directory and removes it on
both success and failure. Before and after each invocation it removes the
legacy checkout `target` directory (including debug, release, coverage, and
fixture artifacts) when no direct Cargo process is using it, so old build
outputs cannot accumulate indefinitely.

```sh
tools/claudex-agent-adapter/scripts/cargo-ephemeral.sh +1.97.1 install \
  --path tools/claudex-agent-adapter \
  --root "$HOME/.cargo" \
  --bin claudex-agent-adapter \
  --bin command-code-acp
```

`create-symlinks.sh` then points `~/.local/bin/claudex-agent-adapter` at that
cargo binary and links `claudex-hot-swap`. Installing only under `~/.local`
leaves a stale cargo bin after the symlink is created. A successful
`cargo-ephemeral.sh … install` (or `scripts/claudex-install-adapter` /
`claudex install`) relinks `~/.local/bin` and arms the idle hot-swap waiter
for the new `build_id`. See [Daemon update and hot-swap](#daemon-update-and-hot-swap).

Use the same wrapper for verification, for example:

```sh
tools/claudex-agent-adapter/scripts/cargo-ephemeral.sh +1.97.1 test \
  --manifest-path tools/claudex-agent-adapter/Cargo.toml
```

The public CLI uses explicit subcommands:

```text
claudex-agent-adapter launch --model MODEL --backend-route MODEL=BACKEND [...] [ADAPTER OPTIONS] [--inherit-claude-model] -- [CLAUDE OPTIONS]
claudex-agent-adapter launch --provider-config PATH [--model MODEL] [ADAPTER OPTIONS] [--inherit-claude-model] -- [CLAUDE OPTIONS]
claudex-agent-adapter ensure --model MODEL --backend-route MODEL=BACKEND [...] [ADAPTER OPTIONS]
claudex-agent-adapter hot-swap --model MODEL --backend-route MODEL=BACKEND [...] [ADAPTER OPTIONS]
claudex-agent-adapter serve --model MODEL --backend-route MODEL=BACKEND [...] [ADAPTER OPTIONS]
claudex-agent-adapter build-id
```

Backend values are `codex-app-server`, `configured-acp`, `copilot-acp`, and
`grok-acp`. The preferred launcher path is `--provider-config
$HOME/.config/claudex/providers.json`. It defines enabled providers, default
models, effort, model prefixes, quota names, fallback, ACP launch settings, and
the legacy/worker-compatibility `mainProviders` list. `mainProviders` does not
select or remap the model in a main-session request. A `configured-acp` provider also supplies a program and argument array;
`{model}` placeholders are replaced directly without invoking a shell.
`--backend-route` is repeatable, model keys must be unique, and the main
`--model` must have a route.
Codex app-server routes may additionally set `modelProvider` and
`modelCatalogJson`. These values are applied per `thread/start`, allowing
OpenAI GPT and custom-provider models such as Sakana Fugu to coexist in the
same persistent app-server process.
Route-specific `maxContextTokens` limits are evaluated only when the request's
actual model selects that route; merely declaring a provider or listing it in
`mainProviders` does not constrain Claude or another provider's request.

Each route may also set `webSearchMode` to `codex-native`, `acp-native`,
`delegate-ccr`, `delegate-mcp`, or `disabled`. `codex-native` enables the
Codex app-server live search flags on `thread/start`; `acp-native` leaves the
request on an ACP route that owns native search (Command Code Muse Spark uses
this so Claude system/routing/ACP_NATIVE dumps are not prefixed onto `cmd -p`;
`command-code-acp` also reads `cmd -p` stdout as bytes so invalid UTF-8 from
web/tool dumps cannot crash the ACP turn, coalesces tiny NDJSON deltas, emits
the same ACP `▶ name: query/path/url` / `✓` / `✗` chrome as Cursor/Qwen/Grok/Cline,
plus native Command Code `text_delta` as live assistant text — not canned
ツール結果待ち / 続きの調査または回答, and not thinking chrome so Claude Code 2.1
does not collapse mid-turn work into Doing/Orbiting);
`delegate-mcp` leaves search
to the configured ACP/MCP provider; and `disabled` suppresses search for that
route. `delegate-ccr` (the default) exposes the protected
`/v1/code/sessions/{session_id}/worker/web-search` endpoint. The endpoint
starts the ordered provider IDs in the top-level `webSearch.fallbackProviders`
array, using each provider's configured model and effort, and returns the
results to the original Claude Code session. It does not change the model that
writes the final answer. This keeps model IDs and effort values configuration-
driven and lets a non-search-capable worker use a separately configured search
worker.

The fish launcher sets `CLAUDE_CODE_WEBSEARCH_USE_CCR_PROXY`,
`CLAUDE_CODE_SESSION_ID`, and `CLAUDE_CODE_SESSION_ACCESS_TOKEN` only for the
outer `claudex` invocation. Subscription children explicitly clear those
local CCR variables, preventing recursive search calls. A fallback worker must
be enabled and have a valid route; configuration loading rejects unknown or
disabled fallback IDs before serving requests.
Omitting all routes preserves the single-model `codex-app-server` default.
Other adapter options are `--listen`, `--subscription-max-processes`, and
`--subscription-timeout-minutes`; their defaults are `127.0.0.1:8318`, 20, and
120. The launch-only `--inherit-claude-model` option omits Claude Code's
`--model` argument, allowing the normal Claude `model` and `effortLevel`
settings to determine the outer conversation's actual request values. The fish
launcher enables this inheritance by default and marks the outer process so the
global, claudex-only routing hook can inject worker capacity context without
selecting or rewriting the main model, changing the session display name, or
introducing a hidden bootstrap route. An explicit `--agent` is still passed
through unchanged. On `--resume`, when a transcript still has the legacy
`agent-setting` value `claudex-orchestrator` and the caller did not pass
`--name`/`--agent`, the launcher restores a display name via `--name` from
`customTitle`, `agentName`, `slug`, or the cwd basename so resume does not keep
showing the orchestrator agent label. `CLAUDEX_MODEL` explicitly disables inheritance and selects
the outer request model, which may be native Claude or any model accepted by a
configured provider prefix. The agent reads a
sanitized, five-minute Codexbar/Qwen Cloud capacity cache and delegates to available agents in
the shared provider config. It selects the configured subscription fallback only
when all capacity-managed providers are unavailable. Claude Code's built-in parameterless
`advisor()` remains independent of provider capacity; the adapter reads its model from
`.claude/settings.json`'s `advisorModel`. The separate `custom-advisor` SubAgent
(`claude-fable-5` / `xhigh`) also stays outside worker capacity accounting and is reused as a
logical session singleton via `SendMessage` (prefer one continuing advisor; not a hard
process=1 OS cap); it coexists with built-in `advisor()` and does not replace it. Set
`CLAUDEX_CUSTOM_ADVISOR` to `0`, `false`, or `off` to skip only custom-advisor launches. The fish launcher translates optional
`CLAUDEX_PROVIDER_CONFIG`, `CLAUDEX_DEFAULTS_SOURCE`, `CLAUDEX_MODEL`,
`CLAUDEX_EFFORT`, `CLAUDEX_ADAPTER_LISTEN`,
`CLAUDEX_SUBAGENT_MIN_PARALLEL`, `CLAUDEX_SUBAGENT_ACTIVE_FLOOR`,
`CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS`, and
`CLAUDEX_SUBAGENT_MIN_MODEL_FAMILIES`,
`CLAUDEX_SUBSCRIPTION_MAX_PROCESSES`, and
`CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES` values into these options. Adapter-private
variables are removed before Claude Code starts. The frequently changed outer
model/effort pair is persisted separately in the gitignored
`~/.config/claudex/defaults.local.json`; omit its `source` (or set it to
`settings`) to inherit `.claude/settings.json`, or set `source` to `explicit`
for its `model` and `effort` values.

For `--resume` or `--continue` without an explicit `CLAUDEX_MODEL`, the launcher
sets `CLAUDEX_MAIN_MODEL_KNOWN=0`. The resumed request's actual `model` remains
authoritative; routing does not fall back to a current or stale settings model,
and worker selection does not suppress a candidate based on assumed model
equality. An explicit `CLAUDEX_MODEL` is the only launcher override that makes
the resumed main model known for equality-based suppression.

The launcher also merges a reserved, percent-encoded working-directory header into
`ANTHROPIC_CUSTOM_HEADERS`. The loopback adapter canonicalizes that path per request and uses it
directly for Codex and subscription subprocesses. ACP session creation first honors a valid
working-directory marker in the request's `baseInstructions`, then the explicit request `cwd`
populated from this header, and finally the daemon launch directory. This preserves ACP's tested
request-instruction semantics without making the dotfiles directory an implicit default. Existing
custom headers are preserved, but an incoming value using the reserved header name is replaced so
a stale or forged header cannot win.

For example, this selects another model dynamically while its matching
`modelPrefixes` entry determines the backend:

```fish
CLAUDEX_MODEL=MODEL claudex
```

`ANTHROPIC_AUTH_TOKEN` remains an environment variable because command-line
secrets are exposed in process listings. API routes accept it as either a
Bearer token or `x-api-key`; `/health` remains public. A non-loopback listener
requires a non-default token. Either `--model` or `--provider-config` is
required for process configuration. The fish function uses the shared config by
default, but `mainProviders` remains a legacy/worker compatibility field and does
not override the model carried by an HTTP request. `CLAUDEX_MODEL` selects the
outer request model when one of the configured prefixes supports it.

Each request selects effort independently. An explicit Anthropic
`output_config.effort` wins; otherwise the adapter rereads Claude Code's
`effortLevel` setting for that request. For an Agent child, an explicit effort
in the Agent tool input overrides the outer request's inherited effort. The
adapter also exposes a private `claudex_effort` field to the main model so a
conversational SubAgent effort request can be captured even when Claude Code's
native Agent schema has no effort field; `mid` is normalized to `medium`, and
the private field is removed before Claude Code executes the Agent. An
unspecified Agent effort uses the current Claude Code setting instead. The same
resolution applies to subscription subprocesses and same-model Codex
app-server child turns, independently of the parent turn. Grok ACP effort is
launch-scoped: the configured `grok-4.5` / `high` route starts with
`--reasoning-effort high`; it is not deferred to `session/set_model` metadata or
the prompt. Every routed native-Grok request therefore resolves and logs its
observable effective effort as the configured launch value `high`, rather than
reporting an unapplied per-turn override. Configured ACP providers, including
OpenCode, keep their existing per-session ACP effort configuration and are not
subject to native-Grok normalization. Copilot ACP receives low, medium, high,
xhigh, or max through ACP session-model metadata.

Each Messages request uses its actual model: a configured Codex model uses the
persistent app-server, a configured ACP model uses its ACP route, and a native
Claude model uses a separate `claude --print` subscription process. There is no
special configured-main-model remap. A Claude Code Agent that explicitly requests
a different model follows the same rule with that request's model and effort.
The subscription child process has the local Anthropic routing variables
removed, so a Sonnet Agent does not merely display a Sonnet label while still
running on the Codex model. It loads the normal Claude Code configuration and
enables and pre-authorizes only tools present in the outer request. This keeps
built-in, configured MCP, and custom tools available to noninteractive Agents
without granting tools that the outer harness did not supply. Existing Claude
Code deny rules still take precedence. The subprocess working directory uses the canonicalized
reserved request header when present and falls back to Claude Code's request environment section.
The adapter accepts an arbitrary explicit SubAgent model through its private
`claudex_model` Agent field, so selection is not limited by Claude Code's native
Agent model enum. It removes provider model details from the public tool input,
correlates the child request, and routes the selected ID through the configured
backend routes. Models matching a configured `modelPrefixes` value are added
lazily and routed through that provider, so manually added model families and
ACPs need no Rust change. When prefixes overlap, the longest matching prefix
wins. Legacy `gpt` and `grok` inference remains available for direct CLI routes.
The launcher prefers a gitignored per-machine denylist
(`disabled-subagent-models.$(hostname -s).local.json`, then
`disabled-subagent-models.local.json`) and falls back to the tracked empty
`~/.config/claudex/disabled-subagent-models.json`. A terminal can select another dedicated file with
`CLAUDEX_DISABLED_SUBAGENT_MODELS_CONFIG` and add comma-separated entries with
`CLAUDEX_DISABLED_SUBAGENT_MODELS`. The launcher sends the merged policy as a reserved per-request
header so terminals sharing one daemon remain independent. The adapter rejects a resolved disabled
SubAgent model before starting its provider; outer main-session and advisor requests remain unaffected.
Live `usage-routing.json` also treats weekly/five-hour remaining below 25% as exhausted when
another worker still has at least 40% headroom, so explicit `claudex-gpt-spark` launches rewrite
onto an ACP sibling instead of burning a depleted Spark quota.
Other user-explicit, genuinely unconfigured model IDs use the Claude subscription
process. A correlated Claude Code child uses the exact `claudex_model` recorded
by its Agent/Task launch. If that metadata is absent or cannot be correlated,
the adapter routes from the request model instead: configured provider models use
their exact provider and native Claude models use the subscription process. A
declared provider that is unavailable returns an error; it does not remap to a
configured main model, another provider, or Claude subscription.

Agent Teams remains controlled by Claude Code. The adapter preserves named
Agent arguments and distinguishes persistent mailbox teammates from regular
background Agents using the Agent tool result. Mailbox teammate IDs are never
treated as `TaskOutput` IDs. Asynchronous task notifications may replay the
Agent's already-consumed `tool_result`; the owning session recognizes that
replay, forwards only the new notification text, and never responds to the same
app-server tool call twice.

Sessions and subscription processes are bounded. Abandoned external tool
requests expire after 30 minutes and receive a failed JSON-RPC result before
their session slot is released. By default, up to 20 subscription subprocesses
may run concurrently and each has a 120-minute timeout. Set
`CLAUDEX_SUBSCRIPTION_MAX_PROCESSES` or
`CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES` before invoking `claudex` to override
either positive integer independently. Subprocesses are killed if their task is
dropped.
Idle provider threads are retained for two hours to support related provider-backed worker
continuations and prompt-prefix reuse; capacity pressure may evict the oldest idle thread sooner.
This backend-thread retention is separate from Claude Code's logical agent lifecycle. The main
session reuses a compatible logical worker by setting `resume` to the exact prior Agent/Task
recipient (Agent Teams still uses `SendMessage`), while starting new instances when the prior
worker failed/stopped or the scope is independent. Prefer one continuing custom-advisor per session
and account for it separately from `selected_workers` / provider quota headroom.
Claude subscription workers and advisors still use a new `--no-session-persistence` subprocess per
provider call. Logical-agent reuse can preserve a reusable transcript prefix but does not guarantee
a provider prompt-cache hit.

Claude Code's UI and Agent `resolvedModel` metadata describe the native custom-agent profile. Every
claudex worker fixes the same model in its frontmatter and the shared provider config. The adapter
still treats the correlated `claudex_model` as the effective provider route. Verify
the effective model from the SubAgent JSONL assistant `message.model`, provider sampling logs, or
adapter routing logs. Nested Agent/Task calls remain supported and must apply the current injected
`selected_workers` selection rather than defaulting to generic `claude` or blindly inheriting the
parent provider. Their `subagent_type` must be one of the current `selected_agents`, and their
`claudex_model` must match the same selected worker entry unless the active user explicitly requested
that exact model. Nested work created natively inside Grok remains in the Grok ACP
session. Cross-provider work is initiated by main orchestration as an explicit routed
Agent/Task, then returns to the main session for integration.

## Daemon update and hot-swap

End-user commands and the daily update workflow are in the
[Claudex guide](../../.config/claudex/README.md#daemonの差し替えhot-swap仕様).
This section is the launcher state machine.

`ensure`, `launch`, and `hot-swap` share `launcher::ensure::run`. A per-port
lock under `~/.cache/claudex` serializes concurrent launchers. Inspection uses
`GET /health` plus a probe request. Compatibility (`ServiceConfig::matches`)
covers protocol, model, Codex/service fingerprints, backend/worker/search
routes, and subscription limits. Freshness is a separate `build_id` comparison
against `env!("CLAUDEX_BUILD_ID")`. Authentication must succeed before Reuse.

| `ServiceState` | When | `ensure` / `launch` | `hot-swap` |
| --- | --- | --- | --- |
| `Reuse` | health matches config, current `build_id`, and auth | keep the listener | keep the listener |
| `Replace` | mismatch or stale build, and no active work | graceful-stop serve, start current binary on the same port | same |
| `Defer` | `status == "ok"` and `has_active_work()` (`active_http_requests` or `active_provider_turns` > 0) | start or reuse a current-build loopback fallback, arm a detached idle waiter for the configured port, and leave in-flight streams on the old pid | poll every 250ms up to 45s; then Replace, or arm `hot-swap --wait-idle` instead of timing out |
| `Start` | no health response | start serve on the configured listen address | same |

Idle `launch` TUIs do **not** block Replace. Only in-flight HTTP/provider work
does. The launcher never signals a `claudex-agent-adapter launch` parent; it
sends SIGTERM only to a matching `serve` pid and does not escalate to the
process group or SIGKILL, so Axum can drain accepted responses. After Replace,
the TUI keeps the same `ANTHROPIC_BASE_URL` and the next turn uses the new
daemon.

Fallback state is `~/.cache/claudex/fallback.<configured-port>.json` (mode
`0600`): listen address, `build_id`, fingerprint, pid. A matching live fallback
is reused. Pending idle hot-swap state is
`~/.cache/claudex/pending-hot-swap.<listen>.json` plus a waiter log. A live
waiter for the current `build_id` is reused; a stale waiter is SIGTERM'd and
replaced. The waiter does not hold the per-port launcher lock while polling.
Arming a new idle waiter posts a macOS notification that the build is waiting;
a successful Replace posts a second notification that the swap completed.
Reuse, an already-armed waiter, and a repeat of the same listen+build+kind do
not notify. Waiting followed by complete is at most two notifications.
Readiness still requires the current build ID, matching
configuration, and successful authentication. If the new generation fails
readiness and a recovery manifest exists, the previous generation is restored.

User-facing wrappers: fish `claudex hot-swap` → `claudex-hot-swap.fish`; POSIX
`scripts/claudex-hot-swap` linked to `~/.local/bin/claudex-hot-swap` for zsh.
Both invoke `claudex-agent-adapter hot-swap --provider-config … --listen …`.
`scripts/claudex-install-adapter` / fish `claudex install` run
`cargo-ephemeral.sh … install`, which calls `after-install.sh` to relink
`~/.local/bin` and arm the idle waiter for the new `build_id`. Canonical binary
is `~/.cargo/bin/claudex-agent-adapter`; `~/.local/bin` is a symlink created by
`create-symlinks.sh`.

`launch --model MODEL -- ...` scopes Anthropic routing,
removes conflicting provider and adapter variables, launches Claude Code with
untouched non-model arguments, and returns Claude Code's exit status. With
`--inherit-claude-model`, it does not inject a main-model argument. It suppresses only the adapter-specific advisor-rank
warning, and returns Claude Code's exit status. Claude Code's
`CLAUDE_CODE_ALWAYS_ENABLE_EFFORT` stays in fish because it is harness UI policy,
not transport configuration. Health checks fail if the selected backend child
exits.

Set `RUST_LOG=debug` when protocol diagnostics are needed. Debug request logs
include only sizes, tool counts, streaming mode, and effort configuration—not
prompt contents.

Development commands:

```sh
env -u RUSTUP_TOOLCHAIN cargo +1.97.1 fmt-check
env -u RUSTUP_TOOLCHAIN cargo +1.97.1 lint
env -u RUSTUP_TOOLCHAIN cargo +1.97.1 test-all
env -u RUSTUP_TOOLCHAIN cargo +1.97.1 coverage
env -u RUSTUP_TOOLCHAIN cargo +1.97.1 coverage-branch
```

`cargo coverage` is the Rust 1.97.1 Cargo entry point to the nightly branch coverage
gate: it runs the `coverage-branch` binary, which explicitly invokes
`cargo +nightly llvm-cov`. Consequently, the exact 1.97.1 command above enforces
at least 95% for all four aggregate metrics—lines, functions, regions, and
branches—plus at least 95% line coverage for every production source file.
`cargo coverage-branch` invokes that gate directly. Branch outcomes generated
more than once for the same source location across unit and integration binaries
are merged before the percentage is calculated. Test-only modules, structural
module-wiring files, and mock process fixtures under `tests/fixtures` are
excluded so the report measures executable production behavior. The ACP client
trait shim, Command Code ACP Agent trait shim, and deterministic Grok plugin
provisioning wrapper are the only production exclusions; each has a documented
nightly LLVM mapping workaround next to the source while the delegated behavior
remains covered by fixture tests. Both coverage commands include the Cargo build script, whose reusable
logic is measured through `src/build_support.rs`.

Coverage uses an isolated `target/llvm-cov-*` directory. A later coverage run
automatically removes artifacts older than ten minutes, while preserving its
own directory and a directory owned by a live sibling coverage process. This
keeps retained failure diagnostics briefly without allowing old instrumented
build outputs to accumulate indefinitely.

The build also rejects production Rust files over 400 physical lines; dedicated
`tests.rs`, `*_tests.rs`, and `tests/**` files are exempt. Clippy rejects
functions over 80 lines, cognitive complexity over 17, and block nesting deeper
than three.
Build-script logic lives in `src/build_support.rs`, is shared by `build.rs`, and
is covered by dedicated integration tests in addition to strict Clippy checks.
An integration audit rejects local control-flow macros and pins the reviewed
`tokio::select!` count because Clippy intentionally skips macro expansions when
calculating nesting.

Private unit and protocol tests remain beside their implementation under
`src/**`, avoiding public test-only APIs. Cross-process CLI, daemon, HTTP, tool
round-trip, and capacity tests live under `tests/**`; mock executables live
under `tests/fixtures/**`. The production build ID hashes build configuration
and `src` only, while integration tests enforce file-size limits across both
`src` and `tests`.

Development and test profiles use incremental compilation with reduced debug
information. Dependencies enable only required features. Release builds optimize for size, abort
on panic, strip symbols, and use fat LTO with one codegen unit (measured smaller and faster than
thin LTO for this crate). Distribution builds should name
`--bin claudex-agent-adapter`; an unqualified build also compiles the five test fixture binaries
declared in `Cargo.toml`.
