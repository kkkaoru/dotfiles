#!/usr/bin/env bash
# Source the repository .env (untracked secrets) from any working directory.
# Use: . "$(cd "$(dirname "$0")/../.." && pwd)/scripts/lib/dotfiles-env.sh"
DOTFILES_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
if [ -f "$DOTFILES_ROOT/.env" ]; then
  set -a
  # shellcheck source=/dev/null
  . "$DOTFILES_ROOT/.env"
  set +a
fi
