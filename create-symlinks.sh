#!/bin/bash

set -euo pipefail

DOTPATH=$(cd "$(dirname "$0")" || exit 1; pwd)

link_path() {
  local src="$1"
  local dest="$2"

  if [ -L "$dest" ]; then
    # Already a symlink: refresh target if it drifted
    ln -snfv "$src" "$dest"
    return
  fi

  if [ -e "$dest" ]; then
    echo "skip: ${dest} exists and is not a symlink" >&2
    return
  fi

  mkdir -p "$(dirname "$dest")"
  ln -snfv "$src" "$dest"
}

# Top-level dotfiles (skip .config — handled below so runtime dirs stay intact)
for f in .??*; do
  [ "$f" = ".git" ] && continue
  [ "$f" = ".tool-versions" ] && continue
  [ "$f" = ".config" ] && continue
  link_path "${DOTPATH}/${f}" "${HOME}/${f}"
done

# .config apps
mkdir -p "${HOME}/.config"
if [ -d "${DOTPATH}/.config" ]; then
  for app_path in "${DOTPATH}/.config"/*; do
    [ -e "$app_path" ] || continue
    name=$(basename "$app_path")
    dest="${HOME}/.config/${name}"

    # Tools that keep runtime state under ~/.config/<app> — link config only
    case "$name" in
      hunk|herdr)
        mkdir -p "$dest"
        if [ -f "${app_path}/config.toml" ]; then
          link_path "${app_path}/config.toml" "${dest}/config.toml"
        fi
        ;;
      *)
        link_path "$app_path" "$dest"
        ;;
    esac
  done
fi
