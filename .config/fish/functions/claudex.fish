function claudex --description 'Run Claude Code with GPT-5.6 Sol through Codex app-server'
    # Keep user-facing Claude Code policy outside the transport adapter.
    set -lx CLAUDE_CODE_ALWAYS_ENABLE_EFFORT 1

    set -l model gpt-5.6-sol
    if set -q CLAUDEX_MODEL
        set model $CLAUDEX_MODEL
    end

    set -l adapter_args launch --model "$model"
    if set -q CLAUDEX_ADAPTER_LISTEN
        set -a adapter_args --listen "$CLAUDEX_ADAPTER_LISTEN"
    end
    if set -q CLAUDEX_SUBSCRIPTION_MAX_PROCESSES
        set -a adapter_args --subscription-max-processes "$CLAUDEX_SUBSCRIPTION_MAX_PROCESSES"
    end
    if set -q CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES
        set -a adapter_args --subscription-timeout-minutes "$CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES"
    end

    command "$HOME/.local/bin/claudex-app-server-adapter" $adapter_args -- $argv
end
