function claudex --description 'Run Claude Code with dynamically routed agent backends'
    # Keep user-facing Claude Code policy outside the transport adapter.
    set -lx CLAUDE_CODE_ALWAYS_ENABLE_EFFORT 1

    # Keep model IDs at the launcher boundary instead of hard-coding providers in Rust.
    set -l codex_model gpt-5.6-sol
    if set -q CLAUDEX_CODEX_MODEL
        set codex_model $CLAUDEX_CODEX_MODEL
    end
    set -l grok_model grok-4.5
    if set -q CLAUDEX_GROK_MODEL
        set grok_model $CLAUDEX_GROK_MODEL
    end

    set -l model $codex_model
    if set -q CLAUDEX_MODEL
        set model $CLAUDEX_MODEL
    end

    set -l adapter_args launch --model "$model"
    set -a adapter_args --backend-route "$codex_model=codex-app-server"
    set -a adapter_args --backend-route "$grok_model=grok-acp"
    if set -q CLAUDEX_ADAPTER_LISTEN
        set -a adapter_args --listen "$CLAUDEX_ADAPTER_LISTEN"
    end
    if set -q CLAUDEX_SUBSCRIPTION_MAX_PROCESSES
        set -a adapter_args --subscription-max-processes "$CLAUDEX_SUBSCRIPTION_MAX_PROCESSES"
    end
    if set -q CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES
        set -a adapter_args --subscription-timeout-minutes "$CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES"
    end

    command "$HOME/.local/bin/claudex-agent-adapter" $adapter_args -- $argv
end
