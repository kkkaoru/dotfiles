# Native Claude Agent PTY acceptance

This opt-in harness verifies claudex through the real Claude Code terminal UI. It is not part of
the default CI suite because it requires local authentication and live model providers.

The harness is pinned to Claude Code `2.1.220`. It verifies:

- one synthetic native background Agent appears in the prompt Agent panel;
- `/tasks` shows that task with an observable state;
- the main prompt accepts and answers a new request while the tasks remain active;
- known claudex regressions such as empty subscription exits, content-block errors, invisible
  background continuation notices, and Agent request timeouts do not appear.

It deliberately does not use `/agents` as running-state evidence. Claude Code `2.1.220` exposes
running work through the prompt Agent panel, `/tasks`, and individual Agent transcripts.

## Deterministic harness tests

```sh
cd tools/claudex-agent-adapter
cargo test --test native_claude_agent_ui
```

This command tests version parsing, terminal-control normalization, UI evidence matching, and
known-error detection without launching Claude Code.

## Real new-session acceptance

```sh
CLAUDEX_RUN_NATIVE_AGENT_UI=1 \
CLAUDEX_NATIVE_AGENT_WORKDIR="$PWD" \
cargo test --test native_claude_agent_ui \
  real_native_agent_ui_tasks_and_prompt_responsiveness -- --ignored --nocapture
```

## Exact-resume acceptance

Pass the resume ID through the environment rather than recording a user session ID in the test:

```sh
CLAUDEX_RUN_NATIVE_AGENT_UI=1 \
CLAUDEX_NATIVE_AGENT_WORKDIR=/absolute/path/to/the/project \
CLAUDEX_NATIVE_AGENT_RESUME_ID=<resume-id> \
cargo test --test native_claude_agent_ui \
  real_native_agent_ui_tasks_and_prompt_responsiveness -- --ignored --nocapture
```

Optional overrides:

- `CLAUDEX_NATIVE_AGENT_COMMAND`: claudex executable, default `claudex`.
- `CLAUDEX_NATIVE_CLAUDE_COMMAND`: Claude Code executable used for the version gate, default
  `claude`.
- `CLAUDEX_NATIVE_AGENT_STARTUP_TIMEOUT_SECONDS`: initial prompt deadline, default 60.
- `CLAUDEX_NATIVE_AGENT_LAUNCH_TIMEOUT_SECONDS`: native Agent launch deadline, default 180.
- `CLAUDEX_NATIVE_AGENT_RESPONSE_TIMEOUT_SECONDS`: `/tasks` and main response deadline, default 60.
- `CLAUDEX_NATIVE_AGENT_ARTIFACT_DIR`: explicitly save the normalized PTY transcript with mode
  `0600`. The transcript can contain resumed-session material; keep it outside the repository and
  do not commit it.

Every run is bounded. The PTY process group is terminated and reaped on success, failure, or panic.
