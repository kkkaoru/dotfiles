# Prefer Homebrew tools before Apple system binaries.
# brew shellenv/path_helper can leave /usr/bin first; prepend Homebrew after.
if [ -x /opt/homebrew/bin/brew ]; then
  eval "$(/opt/homebrew/bin/brew shellenv)"
  export PATH="/opt/homebrew/bin:/opt/homebrew/sbin:${PATH}"
elif [ -x /usr/local/bin/brew ]; then
  eval "$(/usr/local/bin/brew shellenv)"
  export PATH="/usr/local/bin:/usr/local/sbin:${PATH}"
fi

if [ -f "$HOME/.cargo/env" ]; then
  . "$HOME/.cargo/env"
fi
