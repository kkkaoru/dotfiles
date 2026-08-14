function codex --description "Run Codex with scrollback preserved in Ghostty inside Zellij"
    set -l codex_argv $argv
    set -l codex_monitor_shim /Users/kkk4oru/.agents/skills/agmsg/scripts/drivers/types/codex/codex-shim.sh
    set -l requested_model
    set -l explicit_profile 0
    set -l expect_model 0
    set -l expect_profile 0
    for argument in $argv
        if test $expect_model -eq 1
            set requested_model $argument
            set expect_model 0
            continue
        end
        if test $expect_profile -eq 1
            set explicit_profile 1
            set expect_profile 0
            continue
        end
        switch $argument
            case -m --model
                set expect_model 1
            case '--model=*'
                set requested_model (string replace -- '--model=' '' $argument)
            case -p --profile
                set expect_profile 1
            case '--profile=*'
                set explicit_profile 1
        end
    end

    if test $explicit_profile -eq 0
        switch $requested_model
            case fugu 'fugu-*'
                set -p codex_argv --profile fugu
            case 'glm-5.2:cloud'
                set -p codex_argv --profile ollama-launch-codex-app
        end
    end

    set -l in_zellij 0
    if set -q ZELLIJ
        set in_zellij 1
    else if set -q ZELLIJ_SESSION_NAME
        set in_zellij 1
    end

    set -l in_ghostty 0
    if test "$TERM_PROGRAM" = ghostty
        set in_ghostty 1
    else if set -q GHOSTTY_RESOURCES_DIR
        set in_ghostty 1
    else if string match -qi '*ghostty*' -- "$TERM"
        set in_ghostty 1
    end

    if test $in_zellij -eq 1; and test $in_ghostty -eq 1; and not contains -- --no-alt-screen $argv
        command $codex_monitor_shim --no-alt-screen $codex_argv
    else
        command $codex_monitor_shim $codex_argv
    end
end
