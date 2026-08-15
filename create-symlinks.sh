#!/bin/bash

set -euo pipefail

DOTPATH=$(cd "$(dirname "$0")" || exit 1; pwd)
cd "$DOTPATH"

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
  for dest in "$dest_dir"/*; do
    [ -L "$dest" ] || continue
    local target
    target=$(readlink "$dest")
    case "$target" in
      "$src_dir"/*)
        if [ ! -e "$target" ]; then
          rm -v "$dest"
        fi
        ;;
    esac
  done
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

# Top-level dotfiles (stateful agent/config directories are merged below)
for f in .??*; do
  [ "$f" = ".git" ] && continue
  [ "$f" = ".tool-versions" ] && continue
  [ "$f" = ".config" ] && continue
  [ "$f" = ".agents" ] && continue
  [ "$f" = ".claude" ] && continue
  [ "$f" = ".cursor" ] && continue
  [ "$f" = ".grok" ] && continue
  [ "$f" = ".pi" ] && continue
  # Serena stores runtime state under ~/.serena; link only its config file below.
  [ "$f" = ".serena" ] && continue
  link_path "${DOTPATH}/${f}" "${HOME}/${f}"
done

# Serena keeps logs, caches, downloaded language servers, and project metadata
# beside its global configuration. Keep ~/.serena as a real directory so a
# repository's .serena/project.yml can never appear as ~/.serena/project.yml.
if [ -L "${HOME}/.serena" ]; then
  rm -f "${HOME}/.serena"
fi
mkdir -p "${HOME}/.serena"
# Serena mutates the managed `projects` list, so install a regular copy rather
# than linking its writable global config back into this repository.
if [ -L "${HOME}/.serena/serena_config.yml" ]; then
  rm -f "${HOME}/.serena/serena_config.yml"
fi
cp "${DOTPATH}/.config/serena/serena_config.yml" \
  "${HOME}/.serena/serena_config.yml"

# Claude Code keeps history, sessions, plugins, and caches under ~/.claude.
# Link only repository-managed definitions so those runtime paths remain local.
mkdir -p "${HOME}/.claude"
for file in CLAUDE.md settings.json; do
  if [ -f "${DOTPATH}/.claude/${file}" ]; then
    link_path "${DOTPATH}/.claude/${file}" "${HOME}/.claude/${file}"
  fi
done
for name in agents commands hooks rules skills; do
  if [ -d "${DOTPATH}/.claude/${name}" ]; then
    link_tree "${DOTPATH}/.claude/${name}" "${HOME}/.claude/${name}"
  fi
done

# Shared Agent Skills are the canonical definitions used by pi and mirrored
# into harness-specific skill directories where required.
if [ -d "${DOTPATH}/.agents/skills" ]; then
  link_tree "${DOTPATH}/.agents/skills" "${HOME}/.agents/skills"
fi

# Cursor and Grok keep runtime state beside user skills, so merge only skills.
for harness in cursor grok; do
  if [ -d "${DOTPATH}/.${harness}/skills" ]; then
    link_tree "${DOTPATH}/.${harness}/skills" "${HOME}/.${harness}/skills"
  fi
done

# pi keeps sessions and credentials beside global extensions. Keep ~/.pi as a
# real directory and link only managed configuration and extensions.
if [ -L "${HOME}/.pi" ]; then
  rm -f "${HOME}/.pi"
fi
mkdir -p "${HOME}/.pi/agent/extensions"
pi_models_source="${DOTPATH}/.pi/agent/models.json"
pi_models_dest="${HOME}/.pi/agent/models.json"
if [ -f "$pi_models_source" ]; then
  if [ -e "$pi_models_dest" ] && [ ! -L "$pi_models_dest" ]; then
    if ! cmp -s "$pi_models_source" "$pi_models_dest"; then
      echo "refuse: ${pi_models_dest} differs from ${pi_models_source}" >&2
      exit 1
    fi
    rm -f "$pi_models_dest"
  fi
  link_path "$pi_models_source" "$pi_models_dest"
fi
for extension in agmsg loop; do
  extension_path="${DOTPATH}/tools/pi-${extension}-extension"
  if [ -d "$extension_path" ]; then
    link_path "$extension_path" "${HOME}/.pi/agent/extensions/${extension}"
  fi
done
pi_agmsg_extension="${DOTPATH}/tools/pi-agmsg-extension"

# Register pi as an external agmsg agent type through agmsg's supported plugin
# surface. Trust is path-pinned and must be managed by plugin.sh, never by
# editing agmsg's trust/config files directly.
agmsg_skill="${HOME}/.agents/skills/agmsg"
agmsg_pi_plugin="${pi_agmsg_extension}/agmsg-plugin/pi"
agmsg_pi_dest="${agmsg_skill}/plugins/types/pi"
if [ -x "${agmsg_skill}/scripts/plugin.sh" ] && [ -d "$agmsg_pi_plugin" ]; then
  link_path "$agmsg_pi_plugin" "$agmsg_pi_dest"
  if [ -L "$agmsg_pi_dest" ] && [ "$(readlink "$agmsg_pi_dest")" = "$agmsg_pi_plugin" ]; then
    "${agmsg_skill}/scripts/plugin.sh" trust types/pi
  else
    echo "skip: cannot trust types/pi because ${agmsg_pi_dest} is not the managed symlink" >&2
  fi
fi

# Git 2.54+ config-based hooks invoke this stable per-user path from every repository.
mkdir -p "${HOME}/.local/bin"
link_path "${DOTPATH}/tools/git-hooks/dotfiles-git-quality" \
  "${HOME}/.local/bin/dotfiles-git-quality"
# Prefer Homebrew Git over Apple Xcode Git when ~/.local/bin is early on PATH.
if [ -x /opt/homebrew/bin/git ]; then
  link_path /opt/homebrew/bin/git "${HOME}/.local/bin/git"
elif [ -x /usr/local/bin/git ]; then
  link_path /usr/local/bin/git "${HOME}/.local/bin/git"
fi
# Drop the previous traditional hooksPath install if present.
if [ -L "${HOME}/.local/share/dotfiles/git-hooks" ]; then
  rm -f "${HOME}/.local/share/dotfiles/git-hooks"
fi

# Command Code: always launch via mise Node, not Homebrew Node 26 / bun global.
link_path "${DOTPATH}/scripts/command-code-cmd" "${HOME}/.local/bin/cmd"
link_path "${DOTPATH}/scripts/command-code-cmd" "${HOME}/.local/bin/cmdc"
link_path "${DOTPATH}/scripts/command-code-cmd" "${HOME}/.local/bin/command-code"
link_path "${DOTPATH}/scripts/command-code-cmd" "${HOME}/.local/bin/commandcode"

# The Fish launcher executes Cargo's installed adapter directly. Keep the
# historical local path as a symlink too, so manual invocations cannot select
# an obsolete copied binary.
adapter_link="${HOME}/.local/bin/claudex-agent-adapter"
adapter_target="${HOME}/.cargo/bin/claudex-agent-adapter"
if [ -e "$adapter_link" ] && [ ! -L "$adapter_link" ]; then
  mv -f "$adapter_link" "${adapter_link}.legacy"
fi
ln -snfv "$adapter_target" "$adapter_link"
link_path "${DOTPATH}/scripts/claudex-hot-swap" "${HOME}/.local/bin/claudex-hot-swap"
link_path "${DOTPATH}/scripts/ensure-agmsg-claudex-guard.sh" \
  "${HOME}/.local/bin/ensure-agmsg-claudex-guard"
link_path "${DOTPATH}/scripts/claudex-install-adapter" "${HOME}/.local/bin/claudex-install-adapter"
link_path "${DOTPATH}/scripts/serena-dotfiles-mcp" "${HOME}/.local/bin/serena-dotfiles-mcp"

# .config apps
mkdir -p "${HOME}/.config"
if [ -d "${DOTPATH}/.config" ]; then
  for app_path in "${DOTPATH}/.config"/*; do
    [ -e "$app_path" ] || continue
    name=$(basename "$app_path")
    dest="${HOME}/.config/${name}"

    # Tools that keep runtime state under ~/.config/<app> — link config only
    case "$name" in
      serena)
        # Serena reads ~/.serena/serena_config.yml, linked above.
        ;;
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

# Provider-backed claudex children must not start their own agmsg watchers.
# The agmsg skill is installed outside this repository, so apply the guard
# idempotently when the dotfiles are installed or refreshed.
"${DOTPATH}/scripts/ensure-agmsg-claudex-guard.sh"
