function claudex --description 'Run Claude Code with config-driven agent backends'
    # Keep orchestration policy in exported variables so Claude Code and its
    # routed workers receive the same controls.  Each default is overrideable
    # for one invocation (or by a caller's exported shell configuration).
    set -l max_parallel 40
    set -q CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS; and set max_parallel "$CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS"
    set -q CLAUDEX_SUBAGENT_MAX_PARALLEL; and set max_parallel "$CLAUDEX_SUBAGENT_MAX_PARALLEL"
    set -l min_parallel 3
    set -q CLAUDEX_SUBAGENT_MIN_PARALLEL; and set min_parallel "$CLAUDEX_SUBAGENT_MIN_PARALLEL"
    set -l active_floor 2
    set -q CLAUDEX_SUBAGENT_ACTIVE_FLOOR; and set active_floor "$CLAUDEX_SUBAGENT_ACTIVE_FLOOR"
    set -l reassess_seconds 600
    set -q CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS; and set reassess_seconds "$CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS"
    set -l model_families 2
    set -q CLAUDEX_SUBAGENT_MIN_MODEL_FAMILIES; and set model_families "$CLAUDEX_SUBAGENT_MIN_MODEL_FAMILIES"
    set -l reevaluate_on_completion 1
    set -q CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION; and set reevaluate_on_completion "$CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION"
    set -l reuse_workers 1
    set -q CLAUDEX_SUBAGENT_REUSE; and set reuse_workers "$CLAUDEX_SUBAGENT_REUSE"
    set -l cleanup_on_exit 1
    set -q CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT; and set cleanup_on_exit "$CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT"
    set -l subagent_first 1
    set -q CLAUDEX_SUBAGENT_FIRST; and set subagent_first "$CLAUDEX_SUBAGENT_FIRST"
    set -l status_poll_seconds 15
    set -q CLAUDEX_SUBAGENT_STATUS_POLL_SECONDS; and set status_poll_seconds "$CLAUDEX_SUBAGENT_STATUS_POLL_SECONDS"

    set -lx CLAUDE_CODE_ALWAYS_ENABLE_EFFORT 1
    set -lx CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY 1
    set -lx CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS "$max_parallel"
    set -lx CLAUDEX_SUBAGENT_MAX_PARALLEL "$max_parallel"
    set -lx CLAUDEX_SUBAGENT_MIN_PARALLEL "$min_parallel"
    set -lx CLAUDEX_SUBAGENT_ACTIVE_FLOOR "$active_floor"
    set -lx CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS "$reassess_seconds"
    set -lx CLAUDEX_SUBAGENT_MIN_MODEL_FAMILIES "$model_families"
    set -lx CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION "$reevaluate_on_completion"
    set -lx CLAUDEX_SUBAGENT_REUSE "$reuse_workers"
    set -lx CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT "$cleanup_on_exit"
    set -lx CLAUDEX_SUBAGENT_FIRST "$subagent_first"
    set -lx CLAUDEX_SUBAGENT_STATUS_POLL_SECONDS "$status_poll_seconds"
    set -lx CLAUDEX_ACTIVE 1

    # Keep the frequently changed outer-session defaults outside git.  The
    # resolver reads JSON only (never evaluates it as fish code) and defaults
    # to Claude Code's settings.json.  CLAUDEX_DEFAULTS_SOURCE is a convenient
    # one-shot selector; CLAUDEX_MODEL/CLAUDEX_EFFORT remain one-shot overrides.
    set -l defaults_config "$HOME/.config/claudex/defaults.local.json"
    set -q CLAUDEX_DEFAULTS_CONFIG; and set defaults_config "$CLAUDEX_DEFAULTS_CONFIG"
    set -l defaults_config_argument "$defaults_config"
    if not test -e "$defaults_config"
        set defaults_config_argument -
    else if not test -r "$defaults_config"
        echo "claudex: defaults config is not readable: $defaults_config" >&2
        return 2
    end

    set -l settings_config "$HOME/.claude/settings.json"
    set -l defaults_resolver "$HOME/.config/claudex/resolve-defaults.py"
    # A source checkout can be tested before create-symlinks.sh has installed
    # ~/.config/claudex. Resolve the sibling script from this function's real
    # path in that case, while retaining the normal per-user path above.
    if not test -r "$defaults_resolver"
        set -l function_file (status --current-filename)
        set -l resolved_function_file (realpath "$function_file" 2>/dev/null)
        if test -n "$resolved_function_file"
            set -l function_dir (dirname "$resolved_function_file")
            set -l config_root (dirname (dirname "$function_dir"))
            set defaults_resolver "$config_root/claudex/resolve-defaults.py"
        end
    end
    if not test -r "$defaults_resolver"
        echo "claudex: defaults resolver is not readable: $defaults_resolver" >&2
        return 2
    end

    set -l defaults_output (python3 "$defaults_resolver" "$defaults_config_argument" "$settings_config" 2>&1)
    set -l defaults_status $status
    if test $defaults_status -ne 0
        printf '%s\n' $defaults_output >&2
        return 2
    end
    if test (count $defaults_output) -lt 5
        echo "claudex: defaults resolver returned an incomplete result" >&2
        return 2
    end
    set -l defaults_source "$defaults_output[1]"
    set -l outer_model "$defaults_output[2]"
    set -l outer_effort "$defaults_output[3]"
    set -l settings_model "$defaults_output[4]"
    set -l settings_effort "$defaults_output[5]"
    # Export before the pre-launch routing probe as well as for the adapter and
    # UserPromptSubmit hook.  `CLAUDEX_MAIN_MODEL` is the bootstrap provider
    # route and can differ from the actual settings/explicit outer model.
    set -lx CLAUDEX_OUTER_MODEL "$outer_model"

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

    # The shared JSON is authoritative for provider commands, bootstrap models,
    # model prefixes, worker agents, and fallback selection. Claude Code owns
    # advisorModel and (when selected) the outer model/effort settings.
    set -l adapter_args launch --provider-config "$provider_config"
    set -l main_model
    if test "$defaults_source" = explicit
        # Explicit mode keeps the existing routed-main-model behavior and uses
        # the configured model as both the adapter route and outer model.
        set main_model "$outer_model"
        set -a adapter_args --model "$main_model"
    else
        # Prefer the first capacity-available provider in mainProviders. The
        # routing script shares its cached quota snapshot with the Claude Code
        # hook; if it is unavailable, fall back to the first configured
        # provider deterministically.
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
        # Keep a provider route for the adapter daemon, but let Claude Code
        # inherit model and effort from ~/.claude/settings.json.
        set -a adapter_args --model "$main_model" --inherit-claude-model
    end
    set -q CLAUDEX_ADAPTER_LISTEN; and set -a adapter_args --listen "$CLAUDEX_ADAPTER_LISTEN"
    set -l subscription_max_processes 20
    set -q CLAUDEX_SUBSCRIPTION_MAX_PROCESSES; and set subscription_max_processes "$CLAUDEX_SUBSCRIPTION_MAX_PROCESSES"
    set -a adapter_args --subscription-max-processes "$subscription_max_processes"
    set -l subscription_timeout_minutes 120
    set -q CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES; and set subscription_timeout_minutes "$CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES"
    set -a adapter_args --subscription-timeout-minutes "$subscription_timeout_minutes"

    # Routing is injected by the CLAUDEX_ACTIVE-gated global hook. Avoid a
    # default --agent here: Claude Code persists it as the resumed session's
    # agent setting and replaces the session display name with the agent name.
    set -l claude_args $argv
    set -l has_cli_effort 0
    set -l has_allowed_tools 0
    for argument in $argv
        switch $argument
            case --effort '--effort=*'
                set has_cli_effort 1
            case --allowedTools --allowed-tools '--allowedTools=*' '--allowed-tools=*'
                set has_allowed_tools 1
        end
    end
    # Native web tools are returned by the adapter and executed by this outer Claude Code process.
    # Pre-allow them for claudex only so print and interactive sessions do not report zero searches.
    test $has_allowed_tools -eq 1; or set -p claude_args --allowedTools WebSearch,WebFetch
    # Explicit defaults must be forwarded to Claude Code. In settings mode,
    # only an explicit CLAUDEX_EFFORT override is forwarded; otherwise the
    # inherited settings.json value remains authoritative.
    if test $has_cli_effort -eq 0
        if test "$defaults_source" = explicit
            set -p claude_args --effort "$outer_effort"
        else if set -q CLAUDEX_EFFORT
            set -p claude_args --effort "$outer_effort"
        end
    end
    # Do not inject a fixed Claude Code agent/session name. The routing hook and
    # project instructions provide orchestration policy without changing the
    # user's session label; an explicit --agent argument remains untouched.
    set -lx CLAUDEX_MAIN_MODEL "$main_model"
    if test "$defaults_source" = settings
        echo "claudex: settings-routed orchestration ($provider_config, bootstrap $main_model; settings $settings_model, $settings_effort)" >&2
    else
        echo "claudex: explicit-routed orchestration ($provider_config, $outer_model, $outer_effort)" >&2
    end
    # cargo install uses the user-local prefix selected by this repository's
    # install command, so the launcher and installed binary stay in sync.
    set -l adapter "$HOME/.local/bin/claudex-agent-adapter"
    if not test -x "$adapter"
        echo "claudex: installed adapter is not executable: $adapter" >&2
        return 127
    end
    command "$adapter" $adapter_args -- $claude_args
end
