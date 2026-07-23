function claudex --description 'Run Claude Code with dynamically routed agent backends'
    # Keep user-facing Claude Code policy outside the transport adapter.
    set -lx CLAUDE_CODE_ALWAYS_ENABLE_EFFORT 1

    # Discover defaults from each provider's own configuration. Exact model IDs
    # remain user configuration rather than claudex implementation policy.
    set -l codex_model
    if set -q CLAUDEX_CODEX_MODEL
        set codex_model $CLAUDEX_CODEX_MODEL
    else if test -r "$HOME/.codex/config.toml"
        set codex_model (string match -rg '^model\s*=\s*"([^"]+)"' < "$HOME/.codex/config.toml")[1]
    end
    set -l grok_model
    if set -q CLAUDEX_GROK_MODEL
        set grok_model $CLAUDEX_GROK_MODEL
    else if test -r "$HOME/.grok/config.toml"
        set grok_model (string match -rg '^default\s*=\s*"([^"]+)"' < "$HOME/.grok/config.toml")[1]
    end

    # The adapter still needs one provider route as its daemon bootstrap model.
    # With no arguments Claude itself inherits model/effort from Claude settings.
    set -l main_model $codex_model
    test -z "$main_model"; and set main_model $grok_model
    set -q CLAUDEX_MODEL; and set main_model $CLAUDEX_MODEL

    if test -z "$main_model"
        echo 'claudex: set CLAUDEX_MODEL, or configure ~/.codex/config.toml or ~/.grok/config.toml' >&2
        return 2
    end

    set -l main_backend codex-app-server
    string match -q 'grok*' "$main_model"; and set main_backend grok-acp
    set -q CLAUDEX_BACKEND; and set main_backend $CLAUDEX_BACKEND

    set -l adapter_args \
        launch \
        --model "$main_model" \
        --backend-route "$main_model=$main_backend"
    test -n "$codex_model"; and test "$codex_model" != "$main_model"; and set -a adapter_args --backend-route "$codex_model=codex-app-server"
    test -n "$grok_model"; and test "$grok_model" != "$main_model"; and set -a adapter_args --backend-route "$grok_model=grok-acp"
    set -q CLAUDEX_ADAPTER_LISTEN; and set -a adapter_args --listen "$CLAUDEX_ADAPTER_LISTEN"
    set -q CLAUDEX_SUBSCRIPTION_MAX_PROCESSES; and set -a adapter_args --subscription-max-processes "$CLAUDEX_SUBSCRIPTION_MAX_PROCESSES"
    set -q CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES; and set -a adapter_args --subscription-timeout-minutes "$CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES"

    set -l claude_args $argv
    if test (count $argv) -eq 0
        set -a adapter_args --inherit-claude-model
        set claude_args --agent claudex-orchestrator
    end

    # One-line orientation so Claude Code UI feels intentional, not silent.
    # stderr only — never pollute Claude Code stdin/stdout protocol streams.
    if test (count $argv) -eq 0
        echo "claudex: Claude settings + Codexbar-routed subagents (adapter bootstrap=$main_model)" >&2
    else
        echo "claudex: model=$main_model backend=$main_backend (tools stream as live status lines)" >&2
    end

    command "$HOME/.local/bin/claudex-agent-adapter" $adapter_args -- $claude_args
end
