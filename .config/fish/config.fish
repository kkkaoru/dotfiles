eval (/opt/homebrew/bin/brew shellenv)
fish_add_path --move /opt/homebrew/bin /opt/homebrew/sbin

source (dirname (status -f))/aliases.fish
source (dirname (status -f))/envs.fish
source (dirname (status -f))/binds.fish
source (dirname (status -f))/path.fish

# Hide command execution time in the right prompt.
set -g theme_display_cmd_duration no

set -gx HOMEBREW_GITHUB_API_TOKEN your_token_here
# Shared defaults for plain `claude`, `codex`, and `claudex` launches.  Keep
# caller-provided values, while making the claudex policy available even when
# the wrapper function is not used.
set -q CLAUDEX_SUBAGENT_MAX_PARALLEL; and set -gx CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS "$CLAUDEX_SUBAGENT_MAX_PARALLEL"
set -q CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS; or set -gx CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS 40
set -q CLAUDEX_SUBAGENT_MAX_PARALLEL; or set -gx CLAUDEX_SUBAGENT_MAX_PARALLEL "$CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS"
set -q CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION; or set -gx CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION 1
set -q CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS; or set -gx CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS 600
set -q CLAUDEX_SUBAGENT_REUSE; or set -gx CLAUDEX_SUBAGENT_REUSE 1
set -q CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT; or set -gx CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT 1
set -q CLAUDEX_SUBAGENT_FIRST; or set -gx CLAUDEX_SUBAGENT_FIRST 1
set -q CLAUDEX_SUBAGENT_STATUS_POLL_SECONDS; or set -gx CLAUDEX_SUBAGENT_STATUS_POLL_SECONDS 15

# pnpm
set -gx PNPM_HOME "/Users/kaoru/Library/pnpm"
if not string match -q -- $PNPM_HOME $PATH
  set -gx PATH "$PNPM_HOME" $PATH
end
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
fish_add_path --move $HOME/.local/bin

# Added by Windsurf - Next
fish_add_path /Users/kaoru/.codeium/windsurf/bin

# Added by Antigravity
fish_add_path /Users/kaoru/.antigravity/antigravity/bin

# Added by Antigravity
fish_add_path /Users/kaoru/.antigravity/antigravity/bin

# Added by Windsurf
fish_add_path /Users/kaoru/.codeium/windsurf/bin
