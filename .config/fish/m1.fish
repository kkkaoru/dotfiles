eval (/opt/homebrew/bin/brew shellenv)

source (dirname (status -f))/aliases.fish
source (dirname (status -f))/envs.fish
source (dirname (status -f))/binds.fish

eval (direnv hook fish | source)
eval (anyenv init - | source)

# asdf
# m1 mac
# source /opt/homebrew/opt/asdf/libexec/asdf.fish
# intel mac
# source /usr/local/opt/asdf/libexec/asdf.fish
