# history
function fzf_select_history
    history | fzf --query="$commandline" | read -l result
    and commandline -r $result
end

function fish_user_key_bindings
    bind \cr fzf_select_history
    # bind \cr peco_select_history
end
