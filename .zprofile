# After /etc/zprofile's path_helper, force Homebrew ahead of Apple Git.
if [ -x /opt/homebrew/bin/brew ]; then
  eval "$(/opt/homebrew/bin/brew shellenv zsh)"
  export PATH="/opt/homebrew/bin:/opt/homebrew/sbin:${PATH}"
elif [ -x /usr/local/bin/brew ]; then
  eval "$(/usr/local/bin/brew shellenv)"
  export PATH="/usr/local/bin:/usr/local/sbin:${PATH}"
fi
