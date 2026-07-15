function codex --description "Run Codex with scrollback preserved in Ghostty inside Zellij"
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
        command codex --no-alt-screen $argv
    else
        command codex $argv
    end
end
