eval (/opt/homebrew/bin/brew shellenv)

source (dirname (status -f))/aliases.fish
source (dirname (status -f))/envs.fish
source (dirname (status -f))/binds.fish
source /opt/homebrew/opt/asdf/libexec/asdf.fish

# pnpm
set -gx PNPM_HOME $HOME/Library/pnpm
set -gx PATH "$PNPM_HOME" $PATH
# pnpm end
