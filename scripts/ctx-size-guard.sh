#!/bin/bash
# Keep ctx-owned storage below a bounded physical footprint.
# ctx has no byte-retention setting, so an oversized derived search index is
# reset and rebuilt from the remaining provider-owned histories.

set -euo pipefail

LABEL="com.kkkaoru.ctx-size-guard"
DOTPATH=$(cd "$(dirname "$0")/.." || exit 1; pwd)
SCRIPT="${DOTPATH}/scripts/ctx-size-guard.sh"
PLIST_PATH="${HOME}/Library/LaunchAgents/${LABEL}.plist"
CTX_ROOT="${CTX_DATA_ROOT:-${HOME}/.ctx}"
MAX_BYTES="${CTX_MAX_BYTES:-4294967296}"
LOG_DIR="${HOME}/Library/Logs/ctx-size-guard"
LOG_PATH="${LOG_DIR}/guard.log"
LOCK_DIR="${CTX_ROOT}/.size-guard.lock"

size_bytes() {
  local path="$1"
  local blocks
  blocks=$(du -sk "$path" 2>/dev/null | awk '{print $1}')
  printf '%s\n' "$((blocks * 1024))"
}

validate_root() {
  if [ ! -d "$CTX_ROOT" ] || [ -L "$CTX_ROOT" ]; then
    echo "refuse: ctx root must be an existing real directory: ${CTX_ROOT}" >&2
    exit 1
  fi
  local resolved
  resolved=$(cd "$CTX_ROOT" && pwd -P)
  if [ "$resolved" = "/" ] || [ "$resolved" = "$HOME" ]; then
    echo "refuse: unsafe ctx root: ${resolved}" >&2
    exit 1
  fi
  case "$MAX_BYTES" in
    ''|*[!0-9]*) echo "refuse: CTX_MAX_BYTES must be an integer" >&2; exit 1 ;;
  esac
  if [ "$MAX_BYTES" -lt 1073741824 ]; then
    echo "refuse: CTX_MAX_BYTES must be at least 1 GiB" >&2
    exit 1
  fi
}

rotate_log() {
  if [ -f "$LOG_PATH" ] && [ "$(stat -f %z "$LOG_PATH" 2>/dev/null || echo 0)" -gt 1048576 ]; then
    mv -f "$LOG_PATH" "${LOG_PATH}.1"
  fi
}

current_index_mode() {
  if grep -Eq '^[[:space:]]*mode[[:space:]]*=[[:space:]]*"manual"' "${CTX_ROOT}/config.toml" 2>/dev/null; then
    printf 'manual\n'
  else
    printf 'auto\n'
  fi
}

remove_legacy_store() {
  # ctx 0.26+ documents work.sqlite as inert, owner-managed pre-v0.26 state.
  find "$CTX_ROOT" -maxdepth 1 -type f \( \
    -name 'work.sqlite' -o \
    -name 'work.sqlite-*' -o \
    -name 'work.sqlite.*' \
  \) -delete
}

reset_and_rebuild() {
  local mode="$1"
  local backup_dir backup_config uid
  backup_dir=$(mktemp -d "${TMPDIR:-/tmp}/ctx-size-guard.XXXXXX")
  backup_config="${backup_dir}/config.toml"
  if [ -f "${CTX_ROOT}/config.toml" ]; then
    cp "${CTX_ROOT}/config.toml" "$backup_config"
  fi

  # Stop both ctx's logical writer and its launchd supervisor. A complete root
  # reset also clears interrupted refresh receipts that can otherwise prevent
  # a new active generation from being published.
  if command -v ctx >/dev/null 2>&1; then
    ctx --data-root "$CTX_ROOT" index mode manual >/dev/null 2>&1 || true
  fi
  uid=$(id -u)
  launchctl bootout "gui/${uid}/rs.ctx.daemon" 2>/dev/null || true

  rm -rf "$CTX_ROOT"
  mkdir -m 700 "$CTX_ROOT"
  if [ -f "$backup_config" ]; then
    cp "$backup_config" "${CTX_ROOT}/config.toml"
    chmod 600 "${CTX_ROOT}/config.toml"
  fi

  if command -v ctx >/dev/null 2>&1; then
    ctx --data-root "$CTX_ROOT" setup --no-daemon >/dev/null 2>&1 || true
    if [ "$mode" = "manual" ]; then
      # In manual mode, an explicit import uses a finite worker and exits.
      ctx --data-root "$CTX_ROOT" import --all >/dev/null 2>&1 || true
    else
      ctx --data-root "$CTX_ROOT" index mode auto >/dev/null 2>&1 || true
    fi
  fi
  rm -rf "$backup_dir"
}

enforce_limit() {
  validate_root
  mkdir -p "$LOG_DIR"
  rotate_log

  if ! mkdir "$LOCK_DIR" 2>/dev/null; then
    exit 0
  fi
  trap 'rmdir "$LOCK_DIR" 2>/dev/null || true' EXIT INT TERM

  local before mode after
  mode=$(current_index_mode)

  # With persistent indexing disabled, refresh history during this scheduled
  # run using ctx's finite worker. It exits after publishing one generation.
  if [ "$mode" = "manual" ] && command -v ctx >/dev/null 2>&1; then
    ctx --data-root "$CTX_ROOT" import --all >/dev/null 2>&1 || true
  fi

  before=$(size_bytes "$CTX_ROOT")
  if [ "$before" -le "$MAX_BYTES" ]; then
    exit 0
  fi

  printf '%s oversized bytes=%s limit=%s; resetting derived ctx index\n' \
    "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$before" "$MAX_BYTES" >> "$LOG_PATH"

  remove_legacy_store
  reset_and_rebuild "$mode"

  after=$(size_bytes "$CTX_ROOT")
  printf '%s completed bytes_before=%s bytes_after=%s\n' \
    "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$before" "$after" >> "$LOG_PATH"

  if [ "$after" -gt "$MAX_BYTES" ]; then
    echo "ctx size remains above limit after reset: ${after} bytes" >&2
    exit 1
  fi
}

install_agent() {
  mkdir -p "${HOME}/Library/LaunchAgents" "$LOG_DIR"
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
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:${HOME}/.local/bin</string>
    <key>CTX_MAX_BYTES</key>
    <string>${MAX_BYTES}</string>
  </dict>
  <key>StartCalendarInterval</key>
  <array>
    <dict>
      <key>Hour</key>
      <integer>6</integer>
      <key>Minute</key>
      <integer>0</integer>
    </dict>
    <dict>
      <key>Hour</key>
      <integer>18</integer>
      <key>Minute</key>
      <integer>0</integer>
    </dict>
  </array>
  <key>StandardOutPath</key>
  <string>${LOG_PATH}</string>
  <key>StandardErrorPath</key>
  <string>${LOG_PATH}</string>
</dict>
</plist>
EOF
  chmod 600 "$PLIST_PATH"
  local uid
  uid=$(id -u)
  launchctl bootout "gui/${uid}/${LABEL}" 2>/dev/null || true
  launchctl bootstrap "gui/${uid}" "$PLIST_PATH"
  echo "installed ${PLIST_PATH}"
}

uninstall_agent() {
  local uid
  uid=$(id -u)
  launchctl bootout "gui/${uid}/${LABEL}" 2>/dev/null || true
  rm -f "$PLIST_PATH"
  echo "removed ${LABEL}"
}

case "${1:-}" in
  --install) install_agent ;;
  --uninstall) uninstall_agent ;;
  '') enforce_limit ;;
  *) echo "usage: $0 [--install|--uninstall]" >&2; exit 2 ;;
esac
