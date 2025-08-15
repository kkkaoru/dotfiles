eval (/opt/homebrew/bin/brew shellenv)

source (dirname (status -f))/aliases.fish
source (dirname (status -f))/envs.fish
source (dirname (status -f))/binds.fish
source (dirname (status -f))/path.fish

set -gx HOMEBREW_GITHUB_API_TOKEN your_token_here

# pnpm
# set -gx PNPM_HOME /Users/kaoru/Library/pnpm
# if not string match -q -- $PNPM_HOME $PATH
#     set -gx PATH "$PNPM_HOME" $PATH
# end
# pnpm end
# set -gx PNPM_HOME /Users/kaoru/.local/share/mise/installs/node/18/bin/pnpm
if not string match -q -- $PNPM_HOME $PATH
    set -gx PATH "$PNPM_HOME" $PATH
end

# Added by Windsurf
fish_add_path /Users/kaoru/.codeium/windsurf/bin

# thefuck alias
thefuck --alias | source
set -x GPG_TTY (tty)
mise activate fish | source
