function codex --description "Run Codex"
    __recover_working_directory
    or return 1

    set -l codex_argv $argv
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

    for codex_binary in /opt/homebrew/bin/codex /usr/local/bin/codex
        if test -x "$codex_binary"
            command "$codex_binary" $codex_argv
            return $status
        end
    end
    command codex $codex_argv
end
