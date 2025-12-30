# history
function fzf_select_history
    set -l query (commandline)
    history | fzf --ansi --reverse --height=40% --query="$query" | read -l result
    if test -n "$result"
        commandline -r $result
    end
    commandline -f repaint
end

function fish_user_key_bindings
    bind \cr fzf_select_history
    # bind \cr peco_select_history
end
