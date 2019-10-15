# Functions
function fish_user_key_bindings
    bind \cr peco_select_history
end

# Envs
set -x SHELL /usr/local/bin/fish
set -x PATH /usr/local/opt/openssl/bin $PATH
set -x LDFLAGS "-L/usr/local/opt/openssl/lib"
set -x CPPFLAGS "-I/usr/local/opt/openssl/include"

# Evals
eval (thefuck --alias | source)
eval (anyenv init - | source)

# Aliases
alias ghq-cd='cd (ghq root)/(ghq list | peco)'
alias git-checkout-local='git checkout (git branch|peco)'
alias source-fish='exec $SHELL -l'
alias youtube='mpsyt'
