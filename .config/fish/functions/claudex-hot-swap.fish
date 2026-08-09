function claudex-hot-swap --description 'Replace an idle claudex adapter on the same port, even with a launch TUI attached'
    set -l adapter "$HOME/.local/bin/claudex-agent-adapter"
    if not test -x "$adapter"
        echo "claudex-hot-swap: installed adapter is not executable: $adapter" >&2
        return 127
    end

    set -l listen 127.0.0.1:8318
    set -q CLAUDEX_ADAPTER_LISTEN; and set listen "$CLAUDEX_ADAPTER_LISTEN"

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
        if mkdir -p "$cache_path"; and test -r "$resolver_script"
            set -l effective_provider_config (python3 "$resolver_script" "$default_provider_config" "$provider_override_config" "$scoped_provider_config")
            if test $status -eq 0; and test -n "$effective_provider_config"
                set provider_config $effective_provider_config
            end
        end
    end

    if not test -r "$provider_config"
        echo "claudex-hot-swap: provider config is not readable: $provider_config" >&2
        return 2
    end

    echo "claudex-hot-swap: replacing $listen with $(command "$adapter" build-id) ($provider_config)" >&2
    command "$adapter" hot-swap --provider-config "$provider_config" --listen "$listen"
    or return $status

    set -l health_url "http://$listen/health"
    if command -q curl
        curl --fail --silent "$health_url" | python3 -c 'import json,sys; h=json.load(sys.stdin); print("claudex-hot-swap: ready pid=%s build=%s" % (h.get("pid"), h.get("build_id")))'
    end
end
