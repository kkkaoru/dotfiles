eval (/opt/homebrew/bin/brew shellenv)

source (dirname (status -f))/aliases.fish
source (dirname (status -f))/envs.fish
source (dirname (status -f))/binds.fish
source (dirname (status -f))/path.fish

set -gx HOMEBREW_GITHUB_API_TOKEN your_token_here
