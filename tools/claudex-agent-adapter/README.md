# claudex agent adapter

This local Rust service presents the subset of Anthropic's Messages API used by
Claude Code and routes it to one of three agent backends:

| `--backend-route MODEL=BACKEND` | Backend protocol | Tool runtime |
| --- | --- | --- |
| `codex-app-server` | Codex app-server JSON-RPC | Claude Code tools bridged through Codex |
| `copilot-acp` | GitHub Copilot CLI Agent Client Protocol (ACP) | Copilot CLI agent tools and permission requests |
| `grok-acp` | Grok Build Agent Client Protocol (ACP) | Grok Build agent tools and permission requests |

All routes coexist in one daemon without eagerly starting provider processes.
Each configured backend starts lazily on its first model request and remains
available for reuse for the daemon's lifetime. A model switch or a Claude Code SubAgent request is routed from its Messages API
`model` value, while models without a backend route retain the existing Claude
subscription subprocess behavior.

The Codex backend keeps threads alive while Claude Code executes dynamic tool
calls, then sends Claude Code's `tool_result` blocks back to the pending
app-server request. The Copilot backend launches
`copilot --acp --stdio --model MODEL`; Copilot is a backend choice rather than
a separate model family, so an explicit route such as
`--backend-route MODEL=copilot-acp` sends that model through the authenticated
GitHub Copilot CLI. The Grok backend launches `grok --model MODEL agent stdio`,
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
| `ToolCall` / `ToolCallUpdate` | native `tool_use` cards (display-only; input from `raw_input` + content + locations; output preview from `raw_output` / content) |
| `Plan` | compact plan checklist text |
| xAI SubAgent / retry extensions | short status text |

Provider-owned tools never set `stop_reason=tool_use`, so Claude Code does not
re-execute them. Copilot-native SubAgents inherit the model used to launch the
Copilot ACP server.

Streaming requests return their HTTP response immediately. Each Codex
`item/agentMessage/delta` notification is converted to an Anthropic
`content_block_delta` SSE event instead of being buffered until turn completion.
Subscription subprocesses likewise use Claude Code's `stream-json` output and
forward text deltas as they arrive. Streaming responses open immediately with
`message_start` so Anthropic `ping` SSE events keep Claude Code's ~180s raw-byte
idle watchdog alive while the provider session is still being prepared. During
longer provider silence (for example multi-minute Grok tool or subagent waits),
the adapter also emits zero-width `content_block_delta` heartbeats about every
45s so Claude Code's ~300s decoded-event idle watchdog does not abort the stream.

For `codex-app-server`, the adapter starts `codex app-server` with an isolated
`CODEX_HOME`. Only Codex authentication is copied into that home; Claude Code
remains responsible for tools, hooks, MCP servers, skills, approvals, and
project instructions. `CLAUDEX_CODEX_PROGRAM`, `CLAUDEX_COPILOT_PROGRAM`,
`CLAUDEX_GROK_PROGRAM`, and `CLAUDEX_CLAUDE_PROGRAM` are development-only
executable overrides used by process integration tests.

Build and install with the current stable Rust toolchain:

```sh
env -u RUSTUP_TOOLCHAIN cargo install \
  --path tools/claudex-agent-adapter \
  --root "$HOME/.local" \
  --bin claudex-agent-adapter
```

The public CLI uses explicit subcommands:

```text
claudex-agent-adapter launch --model MODEL --backend-route MODEL=BACKEND [...] [ADAPTER OPTIONS] -- [CLAUDE OPTIONS]
claudex-agent-adapter ensure --model MODEL --backend-route MODEL=BACKEND [...] [ADAPTER OPTIONS]
claudex-agent-adapter serve --model MODEL --backend-route MODEL=BACKEND [...] [ADAPTER OPTIONS]
claudex-agent-adapter build-id
```

Backend values are `codex-app-server`, `copilot-acp`, and `grok-acp`.
`--backend-route` is repeatable, model keys must be unique, and the main
`--model` must have a route.
Omitting all routes preserves the single-model `codex-app-server` default.
Other adapter options are `--listen`, `--subscription-max-processes`, and
`--subscription-timeout-minutes`; their defaults are `127.0.0.1:8318`, 20, and
120. The fish launcher configures provider routes and translates optional
`CLAUDEX_MODEL`, `CLAUDEX_BACKEND`, `CLAUDEX_CODEX_MODEL`, `CLAUDEX_GROK_MODEL`,
`CLAUDEX_ADAPTER_LISTEN`, `CLAUDEX_SUBSCRIPTION_MAX_PROCESSES`, and
`CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES` values into these options. Adapter-private
variables are removed before Claude Code starts.

For example, this keeps model selection independent from backend selection and
routes the selected model through GitHub Copilot CLI for the main session:

```fish
CLAUDEX_MODEL=MODEL CLAUDEX_BACKEND=copilot-acp claudex
```

`ANTHROPIC_AUTH_TOKEN` remains an environment variable because command-line
secrets are exposed in process listings. API routes accept it as either a
Bearer token or `x-api-key`; `/health` remains public. A non-loopback listener
requires a non-default token. The main model is a required CLI option and is
not hard-coded by claudex. The fish function discovers provider defaults from
their own configuration files; `CLAUDEX_MODEL` and the provider-specific
variables override those values. Advisor and collaborator model IDs come from
Claude Code settings.

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
app-server child turns, independently of the parent turn. For Grok ACP, low,
medium, and high are sent through `session/set_model` metadata as
`reasoningEffort`; xhigh is normalized to Grok's highest advertised level,
high. Copilot ACP receives low, medium, high, xhigh, or max through the same ACP
session-model metadata.

Requests for the configured main model use the persistent Codex
app-server. A Claude Code Agent that explicitly requests another model is sent
through a separate `claude --print` subscription process with that request's
model and effort. The child process has the local Anthropic routing variables
removed, so a Sonnet Agent does not merely display a Sonnet label while still
running on the Codex model. It loads the normal Claude Code configuration and
enables and pre-authorizes only tools present in the outer request. This keeps
built-in, configured MCP, and custom tools available to noninteractive Agents
without granting tools that the outer harness did not supply. Existing Claude
Code deny rules still take precedence. The subprocess working directory is
parsed and canonicalized from Claude Code's request environment section.
The adapter accepts an arbitrary explicit SubAgent model through its private
`claudex_model` Agent field, so selection is not limited by Claude Code's native
Agent model enum. It removes provider model details from the public tool input,
correlates the child request, and routes the selected ID through the configured
backend routes. An unconfigured model whose ID starts with `gpt` or `grok` is
also added lazily and routed to Codex
app-server or Grok ACP respectively, so SubAgents may select provider models
that were not named when the daemon started. An explicit `copilot-acp` route
takes precedence over this prefix inference. Other unconfigured model IDs fall
back to the Claude subscription process. Without an explicit model, a matched
Claude Code child inherits the model of the session that launched it; an
otherwise-unmatched child request falls back to the configured main model. This
keeps both Claude Code's Agent display and actual routing from claiming a fixed
Sonnet model for an inherited SubAgent.

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

The adapter's `ensure` command compares the running service's protocol, routes,
and limits with the installed binary. A source-derived build ID remains exposed
for diagnostics, but a protocol-compatible daemon is preserved across builds so
in-flight tool ownership is not lost. It restarts an incompatible service,
manages its log and readiness checks, and prints the matching base URL.
`launch --model MODEL -- ...` scopes Anthropic routing,
removes conflicting provider and adapter variables, launches Claude Code with
untouched non-model arguments, suppresses only the adapter-specific advisor-rank
warning, and returns Claude Code's exit status. Claude Code's
`CLAUDE_CODE_ALWAYS_ENABLE_EFFORT` stays in fish because it is harness UI policy,
not transport configuration. Health checks fail if the selected backend child
exits.

Set `RUST_LOG=debug` when protocol diagnostics are needed. Debug request logs
include only sizes, tool counts, streaming mode, and effort configuration—not
prompt contents.

Development commands:

```sh
env -u RUSTUP_TOOLCHAIN cargo fmt-check
env -u RUSTUP_TOOLCHAIN cargo lint
env -u RUSTUP_TOOLCHAIN cargo test-all
env -u RUSTUP_TOOLCHAIN cargo coverage
env -u RUSTUP_TOOLCHAIN cargo coverage-branch
```

`cargo coverage` enforces at least 95% aggregate line, function, and region
coverage, plus at least 95% line coverage for every production source file.
`cargo coverage-branch` uses nightly-only Rust branch instrumentation and
enforces at least 95% for all four aggregate metrics: lines, functions,
regions, and branches. Test-only modules and mock process fixtures under
`tests/fixtures` are excluded so the report measures production behavior. The
ACP client trait shim is the only production exclusion and has a documented
nightly LLVM mapping workaround in the source; its delegated application logic
remains measured. Both coverage commands include the Cargo build script, whose
reusable logic is measured through `src/build_support.rs`.
The build also rejects production Rust files over 400 physical lines; dedicated
`tests.rs`, `*_tests.rs`, and `tests/**` files are exempt. Clippy rejects
functions over 80 lines, cognitive complexity over 17, and block nesting deeper
than four.
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
information. Dependencies enable only required features. Release builds
optimize for size, abort on panic, strip symbols, and use thin LTO to balance
binary size with link time.
