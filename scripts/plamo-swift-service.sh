#!/usr/bin/env bash
# Run the PLaMo translation server at login and keep it up: the mods then always find it warm
# instead of spawning one on their first translation.
#
#   ./scripts/plamo-swift-service.sh install|uninstall|status|reload
#
# The server never exits on idle (--idle-seconds 0) and drops to the lukewarm state, 60 MB, after a
# minute without a request, so holding it for the whole session costs almost nothing. `reload` puts a
# newly built binary in place and restarts the agent, so an update never needs a manual kill.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
LABEL="com.kkkaoru.plamo-swift"
PLIST="${HOME}/Library/LaunchAgents/${LABEL}.plist"
STATE_DIR="${HOME}/.local/state/plamo-swift"
LOG="${STATE_DIR}/serve.log"
BIN="${HOME}/.local/share/plamo-swift/plamo-swift"
MODEL="${HOME}/.local/share/plamo-translate/plamo-3-translate-0.8b-mlx-4bit"
SOCKET="${HOME}/.local/share/plamo-swift/plamo.sock"
PORT="${PLAMO_PORT:-18555}"

ready() {
  curl -sf -m 2 "http://127.0.0.1:${PORT}/v1/models" >/dev/null 2>&1
}

# `bootout` is asynchronous: a `bootstrap` that follows too soon answers EIO, and a server that
# outlived the job, or one a client spawned on demand, still holds the port. Wait for both.
stop_agent() {
  uid=$(id -u)
  launchctl bootout "gui/${uid}/${LABEL}" 2>/dev/null || true
  for _ in $(seq 1 100); do
    launchctl print "gui/${uid}/${LABEL}" >/dev/null 2>&1 || break
    sleep 0.05
  done
  pkill -f "${BIN} serve" 2>/dev/null || true
  for _ in $(seq 1 100); do
    pgrep -f "${BIN} serve" >/dev/null 2>&1 || break
    sleep 0.05
  done
}

start_agent() {
  uid=$(id -u)
  launchctl bootstrap "gui/${uid}" "$PLIST" 2>/dev/null || {
    sleep 0.5
    launchctl bootstrap "gui/${uid}" "$PLIST"
  }
  for _ in $(seq 1 100); do
    ready && break
    sleep 0.05
  done
}

install_agent() {
  [ -x "$BIN" ] || {
    echo "missing ${BIN}: run ./scripts/setup-plamo-translate.sh first" >&2
    exit 1
  }
  mkdir -p "${HOME}/Library/LaunchAgents" "$STATE_DIR"
  cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>${BIN}</string>
    <string>serve</string>
    <string>--model</string>
    <string>${MODEL}</string>
    <string>--port</string>
    <string>${PORT}</string>
    <string>--socket</string>
    <string>${SOCKET}</string>
    <string>--idle-seconds</string>
    <string>0</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>ThrottleInterval</key>
  <integer>10</integer>
  <key>ProcessType</key>
  <string>Interactive</string>
  <key>StandardOutPath</key>
  <string>${LOG}</string>
  <key>StandardErrorPath</key>
  <string>${LOG}</string>
</dict>
</plist>
EOF
  plutil -lint "$PLIST" >/dev/null
  stop_agent
  start_agent
  if ready; then
    echo "installed ${PLIST}; serving on 127.0.0.1:${PORT}"
  else
    echo "installed ${PLIST}, but 127.0.0.1:${PORT} is not answering; see ${LOG}" >&2
    exit 1
  fi
}

uninstall_agent() {
  uid=$(id -u)
  launchctl bootout "gui/${uid}/${LABEL}" 2>/dev/null || true
  rm -f "$PLIST"
  echo "removed ${LABEL}"
}

status_agent() {
  uid=$(id -u)
  if launchctl print "gui/${uid}/${LABEL}" >/dev/null 2>&1; then
    echo "agent: loaded (${LABEL})"
  else
    echo "agent: not loaded"
  fi
  if ready; then
    pid=$(pgrep -f "${BIN} serve" | head -1)
    echo "server: 127.0.0.1:${PORT} answering, pid ${pid:-?}"
    [ -n "${pid:-}" ] && footprint -p "$pid" 2>/dev/null | grep -E 'phys_footprint' || true
  else
    echo "server: 127.0.0.1:${PORT} not answering"
  fi
}

# Replaces the serving binary with the one just built, then restarts the agent. The copy lands in a
# fresh inode and `mv` swaps it in, so the process that is still serving keeps the file it mapped.
# `pack` runs first because the new binary adopts a page-aligned cache instead of copying 501 MB, and
# it is idempotent when the stamp matches.
reload_server() {
  built="${PLAMO_BUILD:-${SCRIPT_DIR}/../tools/plamo-swift/.build/release/plamo-swift}"
  if [ ! -x "$built" ]; then
    echo "missing ${built}: run (cd tools/plamo-swift && swift build -c release)" >&2
    exit 1
  fi
  install -m 755 "$built" "${BIN}.new" 2>/dev/null || {
    # A first install has neither the directory nor the binary the agent will run.
    mkdir -p "$(dirname "$BIN")" "$STATE_DIR"
    install -m 755 "$built" "${BIN}.new"
  }
  mv -f "${BIN}.new" "$BIN"
  cp -R "$(dirname "$built")/mlx-swift_Cmlx.bundle" "$(dirname "$BIN")/" 2>/dev/null || true
  "$BIN" pack --model "$MODEL" \
    || echo "warning: pack failed; the server falls back to loadArrays" >&2
  # `bootout` + `bootstrap` rather than `kickstart -k`: the plist's ThrottleInterval holds a
  # kickstart back for 10 s, while a fresh registration starts the new binary at once.
  install_agent
  pid=$(pgrep -f "${BIN} serve" | head -1)
  if [ -n "${pid:-}" ]; then
    footprint -p "$pid" 2>/dev/null | grep -E 'phys_footprint' || true
  fi
}

case "${1:-status}" in
  install) install_agent ;;
  uninstall) uninstall_agent ;;
  status) status_agent ;;
  reload) reload_server ;;
  *)
    echo "usage: $0 install|uninstall|status|reload" >&2
    exit 2
    ;;
esac
