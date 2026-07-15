function claudex --description 'Run Claude Code with GPT-5.6 Sol through CLIProxyAPI'
    set -lx ANTHROPIC_BASE_URL http://127.0.0.1:8317
    set -lx ANTHROPIC_AUTH_TOKEN claudex-local
    set -lx CLAUDE_CODE_SUBAGENT_MODEL gpt-5.6-sol
    set -lx CLAUDE_CODE_ALWAYS_ENABLE_EFFORT 1
    set -lx ENABLE_CLAUDEAI_MCP_SERVERS false

    if not curl --silent --fail --output /dev/null \
            --header "Authorization: Bearer $ANTHROPIC_AUTH_TOKEN" \
            "$ANTHROPIC_BASE_URL/v1/models"
        echo 'claudex: starting CLIProxyAPI...' >&2
        if not brew services start cliproxyapi >&2
            echo 'claudex: failed to start CLIProxyAPI' >&2
            return 1
        end

        for attempt in (seq 1 20)
            if curl --silent --fail --output /dev/null \
                    --header "Authorization: Bearer $ANTHROPIC_AUTH_TOKEN" \
                    "$ANTHROPIC_BASE_URL/v1/models"
                break
            end
            sleep 0.25
        end

        if not curl --silent --fail --output /dev/null \
                --header "Authorization: Bearer $ANTHROPIC_AUTH_TOKEN" \
                "$ANTHROPIC_BASE_URL/v1/models"
            echo 'claudex: CLIProxyAPI did not become ready' >&2
            return 1
        end
    end

    command claude --model gpt-5.6-sol $argv 2>| while read -l line
        if not string match --quiet -- '*claude.ai connectors are disabled because*' "$line"
            echo "$line" >&2
        end
    end

    return $pipestatus[1]
end
