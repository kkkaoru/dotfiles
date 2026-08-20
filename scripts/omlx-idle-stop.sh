#!/bin/bash
# Stop omlx-server when no local client is using it.
# Does not stop during DeepSWE/Pier evals, or while a model is still loaded.

set -euo pipefail

OMLX_CLI="${OMLX_CLI:-/Applications/oMLX.app/Contents/MacOS/omlx-cli}"
PORT="${OMLX_PORT:-8891}"
LABEL="com.kkkaoru.omlx-idle-stop"
PLIST_NAME="${LABEL}.plist"
PLIST_PATH="${HOME}/Library/LaunchAgents/${PLIST_NAME}"
DOTPATH=$(cd "$(dirname "$0")/.." || exit 1; pwd)
SCRIPT="${DOTPATH}/scripts/omlx-idle-stop.sh"
LOG_DIR="${HOME}/.omlx/logs"
LOG="${LOG_DIR}/idle-stop.log"

eval_busy() {
  pgrep -q -f 'run_all_113.py' && return 0
  pgrep -q -f '/.local/bin/pier run' && return 0
  return 1
}

has_clients() {
  lsof -nP -iTCP:"${PORT}" -sTCP:ESTABLISHED 2>/dev/null | awk 'NR>1 {print}' | grep -q .
}

loaded_count() {
  curl -sf --max-time 2 "http://127.0.0.1:${PORT}/health" \
    | python3 -c "import json,sys; print(json.load(sys.stdin).get('engine_pool',{}).get('loaded_count',0))" \
    2>/dev/null || echo unknown
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
    <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>StartInterval</key>
  <integer>120</integer>
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

stop_if_idle() {
  if ! curl -sf --max-time 2 "http://127.0.0.1:${PORT}/health" >/dev/null; then
    exit 0
  fi
  if eval_busy; then
    exit 0
  fi
  if has_clients; then
    exit 0
  fi
  count=$(loaded_count)
  if [ "$count" != "0" ]; then
    exit 0
  fi
  "$OMLX_CLI" stop --timeout 30 >/dev/null 2>&1 || true
}

case "${1:-}" in
  --install) install_agent ;;
  --uninstall) uninstall_agent ;;
  *) stop_if_idle ;;
esac
