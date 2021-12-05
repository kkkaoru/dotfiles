# Functions
function fish_user_key_bindings
    bind \cr peco_select_history
end

# Envs
# set -x SHELL /usr/local/bin/fish
set -x LDFLAGS "-L/usr/local/opt/openssl/lib"
set -x CPPFLAGS "-I/usr/local/opt/openssl/include"
set -x ANDROID_HOME /usr/local/share/android-sdk

set -x PATH /usr/local/opt/openssl/bin $PATH
# set -x PATH /usr/local/opt/mysql@5.7/bin $PATH
set -x PATH $PATH:$ANDROID_HOME/tools:$ANDROID_HOME/platform-tools $PATH

# Evals
# Disabled anyenv
# eval (anyenv init - | source)


# asdf
# m1 mac
# source /opt/homebrew/opt/asdf/libexec/asdf.fish
# intel mac
# source /usr/local/opt/asdf/libexec/asdf.fish

eval (/opt/homebrew/bin/brew shellenv)

# Aliases
alias ghq-cd='cd (ghq root)/(ghq list | peco)'
alias source-fish='exec $SHELL -l'
# for git
alias git-checkout-local='bash -c \'git checkout $(git branch | peco)\''
alias git-branch-clean='git checkout master && git branch --merged | grep -v -e master | xargs git branch -d'
alias git-push-origin='git push -u origin head'
alias wget-static='wget --page-requisites --html-extension --convert-links'
set -g fish_user_paths "/usr/local/sbin" $fish_user_paths
set -g fish_user_paths "/usr/local/opt/icu4c/bin" $fish_user_paths
set -g fish_user_paths "/usr/local/opt/icu4c/sbin" $fish_user_paths
set -gx LDFLAGS "-L/usr/local/opt/icu4c/lib"
set -gx CPPFLAGS "-I/usr/local/opt/icu4c/include"
set -gx PKG_CONFIG_PATH "/usr/local/opt/icu4c/lib/pkgconfig"
set -g fish_user_paths "/usr/local/opt/gnu-getopt/bin" $fish_user_paths
