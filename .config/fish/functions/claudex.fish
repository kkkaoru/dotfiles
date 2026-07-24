function claudex --description 'Run Claude Code with config-driven agent backends'
    set -lx CLAUDE_CODE_ALWAYS_ENABLE_EFFORT 1

    set -l provider_config "$HOME/.config/claudex/providers.json"
    set -q CLAUDEX_PROVIDER_CONFIG; and set provider_config $CLAUDEX_PROVIDER_CONFIG
    if not test -r "$provider_config"
        echo "claudex: provider config is not readable: $provider_config" >&2
        return 2
    end

    # The shared JSON is authoritative for provider commands, default models,
    # model prefixes, worker agents, fallback, and advisor selection.
    set -l adapter_args launch --provider-config "$provider_config"
    set -q CLAUDEX_MODEL; and set -a adapter_args --model "$CLAUDEX_MODEL"
    set -q CLAUDEX_ADAPTER_LISTEN; and set -a adapter_args --listen "$CLAUDEX_ADAPTER_LISTEN"
    set -q CLAUDEX_SUBSCRIPTION_MAX_PROCESSES; and set -a adapter_args --subscription-max-processes "$CLAUDEX_SUBSCRIPTION_MAX_PROCESSES"
    set -q CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES; and set -a adapter_args --subscription-timeout-minutes "$CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES"

    set -l claude_args $argv
    if test (count $argv) -eq 0
        set claude_args --agent claudex-orchestrator
        echo "claudex: config-routed orchestrator and subagents ($provider_config)" >&2
    else
        echo "claudex: provider config=$provider_config" >&2
    end

    command "$HOME/.local/bin/claudex-agent-adapter" $adapter_args -- $claude_args
end
