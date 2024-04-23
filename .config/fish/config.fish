eval (/opt/homebrew/bin/brew shellenv)

source (dirname (status -f))/aliases.fish
source (dirname (status -f))/envs.fish
source (dirname (status -f))/binds.fish
source (dirname (status -f))/path.fish
source /opt/homebrew/opt/asdf/libexec/asdf.fish

# pnpm
set -gx PNPM_HOME "/Users/kaoru/Library/pnpm"
if not string match -q -- $PNPM_HOME $PATH
  set -gx PATH "$PNPM_HOME" $PATH
end
# pnpm end
set -gx HOMEBREW_GITHUB_API_TOKEN your_token_here
