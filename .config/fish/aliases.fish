# Aliases
function ghq-cd
    set root (ghq root)
    set dest (ghq list | fzf --ansi --reverse --height=40%)
    if test -n "$dest"
        cd "$root/$dest"
    end
end
# for git
alias git-checkout-local='bash -c \'git checkout $(git branch | peco)\''
alias git-branch-clean='git checkout master && git branch --merged | grep -v -e master | xargs git branch -d'
alias git-push-origin='git push -u origin head'
alias wget-static='wget --page-requisites --html-extension --convert-links'
alias source-fish='exec /opt/homebrew/bin/fish -l'
alias git-commit-claude='claude --dangerously-skip-permissions --model "claude-haiku-4-5" -p "/git-commit-by-feature --push"'
