function claudex --description 'Run Claude Code with dynamically routed agent backends'
    # Keep user-facing Claude Code policy outside the transport adapter.
    set -lx CLAUDE_CODE_ALWAYS_ENABLE_EFFORT 1

    # Keep caller-overridable model defaults at the launcher boundary.
    set -q CLAUDEX_CODEX_MODEL; or set -l CLAUDEX_CODEX_MODEL gpt-5.6-sol
    set -q CLAUDEX_GROK_MODEL; or set -l CLAUDEX_GROK_MODEL grok-4.5
    set -q CLAUDEX_MODEL; or set -l CLAUDEX_MODEL $CLAUDEX_CODEX_MODEL

    set -l adapter_args \
        launch \
        --model "$CLAUDEX_MODEL" \
        --backend-route "$CLAUDEX_CODEX_MODEL=codex-app-server" \
        --backend-route "$CLAUDEX_GROK_MODEL=grok-acp"
    set -q CLAUDEX_ADAPTER_LISTEN; and set -a adapter_args --listen "$CLAUDEX_ADAPTER_LISTEN"
    set -q CLAUDEX_SUBSCRIPTION_MAX_PROCESSES; and set -a adapter_args --subscription-max-processes "$CLAUDEX_SUBSCRIPTION_MAX_PROCESSES"
    set -q CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES; and set -a adapter_args --subscription-timeout-minutes "$CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES"

    command "$HOME/.local/bin/claudex-agent-adapter" $adapter_args -- $argv
end
