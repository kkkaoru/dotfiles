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

    set -l claude_args $argv
    set -l has_explicit_agent 0
    for argument in $argv
        if test "$argument" = --agent; or string match -q -- '--agent=*' "$argument"
            set has_explicit_agent 1
            break
        end
    end
    if test $has_explicit_agent -eq 0
        set -p claude_args --agent claudex-orchestrator
        echo "claudex: config-routed orchestrator and subagents ($provider_config)" >&2
    else
        echo "claudex: provider config=$provider_config" >&2
    end

    command "$HOME/.local/bin/claudex-agent-adapter" $adapter_args -- $claude_args
end
