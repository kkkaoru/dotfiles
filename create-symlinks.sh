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

# Merge managed files into applications that keep runtime state beside config.
# Existing regular files are preserved by link_path instead of being overwritten.
link_tree() {
  local src_dir="$1"
  local dest_dir="$2"

  mkdir -p "$dest_dir"
  for src in "$src_dir"/*; do
    [ -e "$src" ] || continue
    local dest
    dest="${dest_dir}/$(basename "$src")"
    if [ -d "$src" ] && [ -d "$dest" ] && [ ! -L "$dest" ]; then
      link_tree "$src" "$dest"
    else
      link_path "$src" "$dest"
    fi
  done
}

# Top-level dotfiles (.config and .claude are merged below to preserve runtime state)
for f in .??*; do
  [ "$f" = ".git" ] && continue
  [ "$f" = ".tool-versions" ] && continue
  [ "$f" = ".config" ] && continue
  [ "$f" = ".claude" ] && continue
  link_path "${DOTPATH}/${f}" "${HOME}/${f}"
done

# Claude Code keeps history, sessions, plugins, and caches under ~/.claude.
# Link only repository-managed definitions so those runtime paths remain local.
mkdir -p "${HOME}/.claude"
if [ -f "${DOTPATH}/.claude/settings.json" ]; then
  link_path "${DOTPATH}/.claude/settings.json" "${HOME}/.claude/settings.json"
fi
for name in agents commands hooks skills; do
  if [ -d "${DOTPATH}/.claude/${name}" ]; then
    link_tree "${DOTPATH}/.claude/${name}" "${HOME}/.claude/${name}"
  fi
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
      fish)
        if [ -d "$dest" ] && [ ! -L "$dest" ]; then
          link_tree "$app_path" "$dest"
        else
          link_path "$app_path" "$dest"
        fi
        ;;
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
