#!/bin/bash

set -euo pipefail

DOTPATH=$(cd "$(dirname "$0")" || exit 1; pwd)
cd "$DOTPATH"

link_path() {
  local src="$1"
  local dest="$2"

  if [ -L "$dest" ]; then
    # Already a symlink: refresh target if it drifted
    rm -f "$dest"
    ln -s "$src" "$dest"
    printf '%s -> %s\n' "$dest" "$src"
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

# ~/.pi must be a symlink to this repository. An existing real directory
# would hide the managed tree, so refuse instead of silently skipping it.
if [ -e "${HOME}/.pi" ] && [ ! -L "${HOME}/.pi" ]; then
  echo "refuse: ${HOME}/.pi exists and is not a symlink; move it into ${DOTPATH}/.pi first" >&2
  exit 1
fi

# ~/.omlx is the same pattern. Weights stay in the checkout but are gitignored.
if [ -e "${HOME}/.omlx" ] && [ ! -L "${HOME}/.omlx" ]; then
  echo "refuse: ${HOME}/.omlx exists and is not a symlink; move it into ${DOTPATH}/.omlx first" >&2
  exit 1
fi

# Top-level dotfiles (stateful agent/config directories are merged below)
for f in .??*; do
  [ "$f" = ".git" ] && continue
  [ "$f" = ".tool-versions" ] && continue
  [ "$f" = ".config" ] && continue
  [ "$f" = ".agents" ] && continue
  [ "$f" = ".claude" ] && continue
  [ "$f" = ".cursor" ] && continue
  [ "$f" = ".grok" ] && continue
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
if [ -L "${HOME}/.claude" ]; then
  echo "refuse: ${HOME}/.claude is a symlink; keep it as a real directory and merge managed files" >&2
  exit 1
fi
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

# Cursor keeps runtime state beside user skills, so merge only skills.
if [ -L "${HOME}/.cursor" ]; then
  echo "refuse: ${HOME}/.cursor is a symlink; keep it as a real directory and merge managed files" >&2
  exit 1
fi
if [ -d "${DOTPATH}/.cursor/skills" ]; then
  link_tree "${DOTPATH}/.cursor/skills" "${HOME}/.cursor/skills"
fi

# Grok keeps sessions, auth, caches, and binaries under ~/.grok. Link only
# repository-managed config, hooks, and skills so those runtime paths stay
# local. ~/.grok itself must remain a real directory (unlike ~/.pi).
if [ -L "${HOME}/.grok" ]; then
  echo "refuse: ${HOME}/.grok is a symlink; keep it as a real directory and merge managed files" >&2
  exit 1
fi
mkdir -p "${HOME}/.grok"
for file in config.toml pager.toml lsp.json; do
  if [ -f "${DOTPATH}/.grok/${file}" ]; then
    link_path "${DOTPATH}/.grok/${file}" "${HOME}/.grok/${file}"
  fi
done
for name in agents commands hooks plugins skills; do
  if [ -d "${DOTPATH}/.grok/${name}" ]; then
    link_tree "${DOTPATH}/.grok/${name}" "${HOME}/.grok/${name}"
  fi
done

# ~/.pi is a top-level symlink to this repository. Keep repository-owned
# extensions inside the managed tree so a fresh clone still installs them.
mkdir -p "${DOTPATH}/.pi/agent/extensions" "${DOTPATH}/.pi/agent/packages"
for extension in agmsg loop omlx-lifecycle tmux-timeout; do
  extension_path="${DOTPATH}/tools/pi-${extension}-extension"
  if [ -d "$extension_path" ]; then
    link_path "$extension_path" "${DOTPATH}/.pi/agent/extensions/${extension}"
  fi
done
# Pi resolves package-relative paths against ~/.pi/agent without following the
# ~/.pi symlink. Point settings at these local package links so other machines
# can reuse the same relative paths after create-symlinks.sh.
for package in pi-effort-manager pi-my-clinepass-provider pi-my-cursor-provider pi-my-devin-cli-provider pi-claudex-provider pi-lazy-external-extensions; do
  package_path="${DOTPATH}/tools/${package}"
  if [ -d "$package_path" ]; then
    link_path "$package_path" "${DOTPATH}/.pi/agent/packages/${package}"
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
if [ -L "$adapter_link" ]; then
  rm -f "$adapter_link"
fi
ln -s "$adapter_target" "$adapter_link"
printf '%s -> %s\n' "$adapter_link" "$adapter_target"
link_path "${DOTPATH}/scripts/claudex-hot-swap" "${HOME}/.local/bin/claudex-hot-swap"
link_path "${DOTPATH}/scripts/ensure-agmsg-claudex-guard.sh" \
  "${HOME}/.local/bin/ensure-agmsg-claudex-guard"
link_path "${DOTPATH}/scripts/claudex-install-adapter" "${HOME}/.local/bin/claudex-install-adapter"
link_path "${DOTPATH}/scripts/serena-dotfiles-mcp" "${HOME}/.local/bin/serena-dotfiles-mcp"
link_path "${DOTPATH}/scripts/ensure-omlx.sh" "${HOME}/.local/bin/ensure-omlx"
link_path "${DOTPATH}/scripts/omlx-idle-stop.sh" "${HOME}/.local/bin/omlx-idle-stop"
link_path "${DOTPATH}/scripts/pi" "${HOME}/.local/bin/pi"
if [ -x "${DOTPATH}/scripts/omlx-idle-stop.sh" ]; then
  "${DOTPATH}/scripts/omlx-idle-stop.sh" --install
fi

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
      claudex)
        # prepare-claude-config may mkdir ~/.config/claudex as a real directory
        # before this installer runs. Merge tracked files so the denylist is
        # installed instead of skipping the whole tree.
        if [ -d "$dest" ] && [ ! -L "$dest" ]; then
          mkdir -p "$dest"
          for src in "$app_path"/*; do
            [ -e "$src" ] || continue
            name=$(basename "$src")
            case "$name" in
              claude-config|tests|__pycache__)
                continue
                ;;
              *.local.json)
                continue
                ;;
            esac
            link_path "$src" "${dest}/${name}"
          done
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
