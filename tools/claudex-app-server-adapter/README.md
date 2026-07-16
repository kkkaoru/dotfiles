# claudex app-server adapter

This local Rust service translates the subset of Anthropic's Messages API used
by Claude Code into the Codex app-server JSON-RPC protocol. It keeps Codex
threads alive while Claude Code executes dynamic tool calls, then sends Claude
Code's `tool_result` blocks back to the pending app-server request.

Streaming requests return their HTTP response immediately. Each Codex
`item/agentMessage/delta` notification is converted to an Anthropic
`content_block_delta` SSE event instead of being buffered until turn completion.
Subscription subprocesses likewise use Claude Code's `stream-json` output and
forward text deltas as they arrive.

The adapter starts `codex app-server` with an isolated `CODEX_HOME`. Only Codex
authentication is copied into that home; Claude Code remains responsible for
tools, hooks, MCP servers, skills, approvals, and project instructions.
`CLAUDEX_APP_SERVER_PROGRAM` and `CLAUDEX_CLAUDE_PROGRAM` are development-only
executable overrides used by process integration tests.

Build and install with the current stable Rust toolchain:

```sh
env -u RUSTUP_TOOLCHAIN cargo install \
  --path tools/claudex-app-server-adapter \
  --root "$HOME/.local" \
  --bin claudex-app-server-adapter
```

The public CLI uses explicit subcommands:

```text
claudex-app-server-adapter launch --model MODEL [ADAPTER OPTIONS] -- [CLAUDE OPTIONS]
claudex-app-server-adapter ensure --model MODEL [ADAPTER OPTIONS]
claudex-app-server-adapter serve --model MODEL [ADAPTER OPTIONS]
claudex-app-server-adapter build-id
```

Adapter options are `--listen`, `--subscription-max-processes`, and
`--subscription-timeout-minutes`; their defaults are `127.0.0.1:8318`, 20, and
120. The fish launcher translates optional `CLAUDEX_MODEL`,
`CLAUDEX_ADAPTER_LISTEN`, `CLAUDEX_SUBSCRIPTION_MAX_PROCESSES`, and
`CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES` values into these options. Adapter-private
variables are removed before Claude Code starts.

`ANTHROPIC_AUTH_TOKEN` remains an environment variable because command-line
secrets are exposed in process listings. API routes accept it as either a
Bearer token or `x-api-key`; `/health` remains public. A non-loopback listener
requires a non-default token. The main model is a required CLI option and is
not hard-coded in Rust. Advisor and collaborator model IDs come from Claude
Code settings.

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
app-server child turns, independently of the parent turn.

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

The adapter's `ensure` command compares the running service's
protocol, model, limits, and source-derived build ID with the installed binary.
It restarts a stale service, manages its log and readiness checks, and prints
the matching base URL. `launch --model MODEL -- ...` scopes Anthropic routing,
removes conflicting provider and adapter variables, launches Claude Code with
untouched non-model arguments, suppresses only the adapter-specific advisor-rank
warning, and returns Claude Code's exit status. Claude Code's
`CLAUDE_CODE_ALWAYS_ENABLE_EFFORT` stays in fish because it is harness UI policy,
not transport configuration. Health checks fail if the child app-server exits.

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

`cargo coverage` enforces at least 95% line coverage both across all `src`
files and for every individual source file. Mock process fixtures live under
`tests/fixtures` and are excluded as test infrastructure. `cargo
coverage-branch` uses the nightly-only Rust branch instrumentation to enforce
the same line gates and at least 95% aggregate branch coverage. Both coverage
commands include the Cargo build script; its reusable logic is measured through
`src/build_support.rs`.
The build also rejects Rust files over 500 lines; Clippy rejects functions over
80 lines, cognitive complexity over 17, and block nesting deeper than four.
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
