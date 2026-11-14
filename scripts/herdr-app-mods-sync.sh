#!/bin/bash
# Copy this repository's Herdr.app mods into ~/.config/herdr-app/mods as real folders, and keep
# them in step. Herdr hot-reloads a mod when a file under its own mods/ folder changes, but a
# symlinked mod folder hides edits made at the link's target, so changes showed only after a
# restart. Run without arguments, it polls the repository every two seconds and copies a changed mod.

set -euo pipefail

LABEL="com.kkkaoru.herdr-app-mods-sync"
PLIST_PATH="${HOME}/Library/LaunchAgents/${LABEL}.plist"
DOTPATH=$(cd "$(dirname "$0")/.." || exit 1; pwd)
SCRIPT="${DOTPATH}/scripts/herdr-app-mods-sync.sh"
SRC="${DOTPATH}/.config/herdr-app/mods"
DEST="${HERDR_APP_HOME:-${HOME}/.config/herdr-app}/mods"
LOG="${HOME}/Library/Logs/herdr-app-mods-sync.log"
MODS=(prefix spaces-emoji agents-emoji)

# A link left by an older create-symlinks.sh points back here: rsync would write through it into
# the repository, so it is replaced by a folder first.
sync_mod() {
  local mod="$1"
  if [ -L "${DEST}/${mod}" ]; then
    rm "${DEST}/${mod}"
  fi
  mkdir -p "${DEST}/${mod}"
  rsync -a --delete "${SRC}/${mod}/" "${DEST}/${mod}/"
}

sync_all() {
  local mod
  for mod in "${MODS[@]}"; do
    sync_mod "$mod"
  done
}

# Copies a mod only when one of its files is newer than the last pass, so an idle pass costs a
# `find` and Herdr reloads only the mod that changed.
watch() {
  local stamp mod changed
  stamp=$(mktemp)
  sync_all
  while true; do
    sleep 2
    changed=()
    for mod in "${MODS[@]}"; do
      if [ -n "$(find "${SRC}/${mod}" -newer "$stamp" -print -quit)" ]; then
        changed+=("$mod")
      fi
    done
    # Stamped before copying, so an edit made during the copy is seen on the next pass.
    touch "$stamp"
    for mod in "${changed[@]+"${changed[@]}"}"; do
      sync_mod "$mod"
      echo "$(date '+%Y-%m-%dT%H:%M:%S%z') synced ${mod}"
    done
  done
}

install_agent() {
  mkdir -p "${HOME}/Library/LaunchAgents" "$(dirname "$LOG")"
  umask 077
  cat > "$PLIST_PATH" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>${SCRIPT}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>ProcessType</key>
  <string>Background</string>
  <key>StandardOutPath</key>
  <string>${LOG}</string>
  <key>StandardErrorPath</key>
  <string>${LOG}</string>
</dict>
</plist>
EOF
  uid=$(id -u)
  launchctl bootout "gui/${uid}/${LABEL}" 2>/dev/null || true
  launchctl bootstrap "gui/${uid}" "$PLIST_PATH"
  echo "installed ${PLIST_PATH}"
}

uninstall_agent() {
  uid=$(id -u)
  launchctl bootout "gui/${uid}/${LABEL}" 2>/dev/null || true
  rm -f "$PLIST_PATH"
  echo "removed ${LABEL}"
}

case "${1:-}" in
  --install) install_agent ;;
  --uninstall) uninstall_agent ;;
  --once) sync_all ;;
  *) watch ;;
esac
