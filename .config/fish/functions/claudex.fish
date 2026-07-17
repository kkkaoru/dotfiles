function claudex --description 'Run Claude Code with dynamically routed agent backends'
    # Keep user-facing Claude Code policy outside the transport adapter.
    set -lx CLAUDE_CODE_ALWAYS_ENABLE_EFFORT 1

    # Keep model IDs at the launcher boundary instead of hard-coding providers in Rust.
    set -l codex_model gpt-5.6-sol
    set -q CLAUDEX_CODEX_MODEL; and set codex_model $CLAUDEX_CODEX_MODEL
    set -l grok_model grok-4.5
    set -q CLAUDEX_GROK_MODEL; and set grok_model $CLAUDEX_GROK_MODEL

    set -l model $codex_model
    set -q CLAUDEX_MODEL; and set model $CLAUDEX_MODEL

    set -l adapter_args launch --model "$model"
    set -a adapter_args --backend-route "$codex_model=codex-app-server"
    set -a adapter_args --backend-route "$grok_model=grok-acp"
    set -q CLAUDEX_ADAPTER_LISTEN; and set -a adapter_args --listen "$CLAUDEX_ADAPTER_LISTEN"
    set -q CLAUDEX_SUBSCRIPTION_MAX_PROCESSES; and set -a adapter_args --subscription-max-processes "$CLAUDEX_SUBSCRIPTION_MAX_PROCESSES"
    set -q CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES; and set -a adapter_args --subscription-timeout-minutes "$CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES"

    command "$HOME/.local/bin/claudex-agent-adapter" $adapter_args -- $argv
end
