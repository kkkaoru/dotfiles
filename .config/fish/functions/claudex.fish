function claudex --description 'Run Claude Code with config-driven agent backends'
    set -lx CLAUDE_CODE_ALWAYS_ENABLE_EFFORT 1
    set -lx CLAUDEX_ACTIVE 1

    set -l provider_config "$HOME/.config/claudex/providers.json"
    set -q CLAUDEX_PROVIDER_CONFIG; and set provider_config $CLAUDEX_PROVIDER_CONFIG
    if not test -r "$provider_config"
        echo "claudex: provider config is not readable: $provider_config" >&2
        return 2
    end

    # The shared JSON is authoritative for provider commands, default models,
    # model prefixes, worker agents, fallback, and advisor selection.
    set -l adapter_args launch --provider-config "$provider_config"
    if set -q CLAUDEX_MODEL
        # An explicit provider-model override keeps the existing routed-main-model behavior.
        set -a adapter_args --model "$CLAUDEX_MODEL"
    else
        # Normal launches keep the adapter routes but let Claude Code's model and
        # effortLevel settings control the outer orchestrator session.
        set -a adapter_args --inherit-claude-model
    end
    set -q CLAUDEX_ADAPTER_LISTEN; and set -a adapter_args --listen "$CLAUDEX_ADAPTER_LISTEN"
    set -q CLAUDEX_SUBSCRIPTION_MAX_PROCESSES; and set -a adapter_args --subscription-max-processes "$CLAUDEX_SUBSCRIPTION_MAX_PROCESSES"
    set -q CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES; and set -a adapter_args --subscription-timeout-minutes "$CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES"

    # Routing is injected by the CLAUDEX_ACTIVE-gated global hook. Avoid a
    # default --agent here: Claude Code persists it as the resumed session's
    # agent setting and replaces the session display name with the agent name.
    echo "claudex: config-routed orchestration ($provider_config)" >&2
    command "$HOME/.local/bin/claudex-agent-adapter" $adapter_args -- $argv
end
