function claudex --description 'Run Claude Code with config-driven agent backends'
    set -lx CLAUDE_CODE_ALWAYS_ENABLE_EFFORT 1
    set -lx CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY 1
    set -lx CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS 40
    set -lx CLAUDEX_ACTIVE 1

    set -l default_provider_config "$HOME/.config/claudex/providers.json"
    set -l provider_override_config "$HOME/.config/claudex/providers.$(hostname -s).local.json"
    set -l provider_config $default_provider_config

    if set -q CLAUDEX_PROVIDER_CONFIG
        set provider_config $CLAUDEX_PROVIDER_CONFIG
    else if set -q CLAUDEX_PROVIDER_LOCAL_CONFIG
        set provider_override_config $CLAUDEX_PROVIDER_LOCAL_CONFIG
    end

    if test -z "$CLAUDEX_PROVIDER_CONFIG"; and test -r "$provider_override_config"
        set -l cache_path "$HOME/.cache/claudex"
        set -l scoped_provider_config "$cache_path/providers.local-resolved.json"
        set -l resolver_script "$HOME/.config/claudex/resolve-provider-local-config.py"

        if not mkdir -p "$cache_path"
            echo "claudex: cannot create provider cache directory: $cache_path" >&2
            return 2
        end

        set -l effective_provider_config (python3 "$resolver_script" "$default_provider_config" "$provider_override_config" "$scoped_provider_config")
        set -l resolver_status $status
        if test $resolver_status -ne 0
            echo "claudex: failed to resolve local provider config" >&2
            return 2
        end
        if test -z "$effective_provider_config"
            echo "claudex: failed to resolve local provider config" >&2
            return 2
        end
        set provider_config $effective_provider_config
    end

    set -lx CLAUDEX_PROVIDER_CONFIG "$provider_config"
    if not test -r "$provider_config"
        echo "claudex: provider config is not readable: $provider_config" >&2
        return 2
    end

    # The shared JSON is authoritative for provider commands, default models,
    # model prefixes, worker agents, and fallback selection. Claude Code owns advisorModel.
    set -l adapter_args launch --provider-config "$provider_config"
    set -l main_model
    if set -q CLAUDEX_MODEL
        # An explicit provider-model override keeps the existing routed-main-model behavior.
        set main_model "$CLAUDEX_MODEL"
        set -a adapter_args --model "$main_model"
    else
        # Prefer the first capacity-available provider in mainProviders. The routing
        # script shares its cached quota snapshot with the Claude Code hook; if it is
        # unavailable, fall back to the first configured provider deterministically.
        set -l routing_script "$HOME/.claude/skills/claudex-routing/scripts/route_usage.py"
        set -l routing_output
        if test -r "$routing_script"
            set routing_output (env CLAUDEX_PROVIDER_CONFIG="$provider_config" python3 "$routing_script" 2>/dev/null)
        end
        set main_model (printf '%s' "$routing_output" | python3 -c '
import json
import sys

config = json.load(open(sys.argv[1], encoding="utf-8"))
model = None
try:
    hook = json.load(sys.stdin)
    context = hook["hookSpecificOutput"]["additionalContext"]
    summary = json.loads(context[context.index("{\\"providers\\":"):])
    worker = summary.get("preferred_main_worker")
    if isinstance(worker, dict) and isinstance(worker.get("model"), str):
        model = worker["model"]
except (IndexError, KeyError, TypeError, ValueError, json.JSONDecodeError):
    pass
if model is None:
    providers = {provider.get("id"): provider for provider in config.get("providers", [])}
    for provider_id in config.get("mainProviders", []):
        provider = providers.get(provider_id)
        if isinstance(provider, dict) and provider.get("enabled", True):
            candidate = provider.get("defaultModel")
            if isinstance(candidate, str) and candidate:
                model = candidate
                break
if model is None:
    raise SystemExit("claudex: no enabled provider in mainProviders")
print(model)
' "$provider_config")
        if test -z "$main_model"
            return 2
        end
        set -a adapter_args --model "$main_model"
    end
    set -q CLAUDEX_ADAPTER_LISTEN; and set -a adapter_args --listen "$CLAUDEX_ADAPTER_LISTEN"
    set -l subscription_max_processes 40
    set -q CLAUDEX_SUBSCRIPTION_MAX_PROCESSES; and set subscription_max_processes "$CLAUDEX_SUBSCRIPTION_MAX_PROCESSES"
    set -a adapter_args --subscription-max-processes "$subscription_max_processes"
    set -q CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES; and set -a adapter_args --subscription-timeout-minutes "$CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES"

    # Routing is injected by the CLAUDEX_ACTIVE-gated global hook. Avoid a
    # default --agent here: Claude Code persists it as the resumed session's
    # agent setting and replaces the session display name with the agent name.
    set -lx CLAUDEX_MAIN_MODEL "$main_model"
    echo "claudex: config-routed orchestration ($provider_config, $main_model)" >&2
    command "$HOME/.local/bin/claudex-agent-adapter" $adapter_args -- $argv
end
