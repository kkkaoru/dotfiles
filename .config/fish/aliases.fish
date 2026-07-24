# Aliases
function ghq-cd
    set root (ghq root)
    set dest (ghq list | fzf --ansi --reverse --height=40%)
    if test -n "$dest"
        cd "$root/$dest"
    end
end
function worktree-hunk
    set wt (git worktree list | fzf --ansi --reverse --height=40% | awk '{print $1}')
    if test -n "$wt"
        set default_branch (git -C "$wt" symbolic-ref --short refs/remotes/origin/HEAD 2>/dev/null | string replace 'origin/' '')
        if test -z "$default_branch"
            for b in main master
                if git -C "$wt" rev-parse --verify --quiet refs/heads/$b >/dev/null
                    set default_branch $b
                    break
                end
            end
        end
        if test -z "$default_branch"
            echo "worktree-hunk: default branch を特定できません" >&2
            return 1
        end
        env -C "$wt" hunk diff "$default_branch" --watch
    end
end
# for git
alias git-checkout-local='bash -c \'git checkout $(git branch | peco)\''
alias git-branch-clean='git checkout master && git branch --merged | grep -v -e master | xargs git branch -d'
alias git-push-origin='git push -u origin head'
alias wget-static='wget --page-requisites --html-extension --convert-links'
alias source-fish='exec /opt/homebrew/bin/fish -l'
alias git-commit-claude='claude --dangerously-skip-permissions --model "claude-haiku-4-5" -p "/git-commit-by-feature --push"'
alias zj='zellij-cwd'
alias hdr='herdr-cwd'
alias hkw='worktree-hunk'
alias wt='worktree-hunk'
