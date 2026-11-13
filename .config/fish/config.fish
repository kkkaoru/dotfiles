eval (/opt/homebrew/bin/brew shellenv)

# A terminal server (Herdr, tmux) started from a Claude Code Bash tool hands that session's markers
# to every new pane, so `claude` there turns off transcript saving. Claude's own tools run non-interactive shells.
if status is-interactive
    set -e CLAUDECODE CLAUDE_PID CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDE_CODE_SESSION_ATTENDED \
        CLAUDE_CODE_ENTRYPOINT CLAUDE_CODE_EXECPATH CLAUDE_CODE_MESSAGING_SOCKET CLAUDE_CODE_MESSAGING_TOKEN
end
fish_add_path --move /opt/homebrew/bin /opt/homebrew/sbin

source (dirname (status -f))/aliases.fish
source (dirname (status -f))/envs.fish
source (dirname (status -f))/binds.fish
source (dirname (status -f))/path.fish

# Hide command execution time in the right prompt.
set -g theme_display_cmd_duration no

set -gx HOMEBREW_GITHUB_API_TOKEN your_token_here

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
