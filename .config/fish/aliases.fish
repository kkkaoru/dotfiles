# Aliases
alias ghq-cd='cd (ghq root)/(ghq list | peco)'
# for git
alias git-checkout-local='bash -c \'git checkout $(git branch | peco)\''
alias git-branch-clean='git checkout master && git branch --merged | grep -v -e master | xargs git branch -d'
alias git-push-origin='git push -u origin head'
alias wget-static='wget --page-requisites --html-extension --convert-links'
