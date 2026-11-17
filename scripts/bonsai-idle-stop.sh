#!/usr/bin/env bash
# Stop the Bonsai MLX server after BONSAI_IDLE_MINUTES with no request and no client connection:
# the checkpoint holds 8.6 GB and a loaded MLX graph keeps it resident, so it must not idle forever.
#   --install / --uninstall   the launchd agent that runs this check every 5 minutes
#   BONSAI_PORT               port to watch (default: 8090)
#   BONSAI_IDLE_MINUTES       idle window before stopping (default: 10)
set -euo pipefail

LABEL="com.kkkaoru.bonsai-idle-stop"
PLIST="${HOME}/Library/LaunchAgents/${LABEL}.plist"
# The launchd job runs this through ~/.local/bin, so resolve the symlink to the repository copy.
SCRIPT=$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$0")
STATE="${HOME}/.local/state/bonsai"
LOG="${STATE}/idle-stop.log"

port="${BONSAI_PORT:-8090}"
idle_minutes="${BONSAI_IDLE_MINUTES:-10}"

install_agent() {
  mkdir -p "${HOME}/Library/LaunchAgents" "$STATE"
  umask 077
  cat > "$PLIST" <<EOF
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
    <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/Users/$(id -un)/.local/bin</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>StartInterval</key>
  <integer>300</integer>
  <key>StandardOutPath</key>
  <string>${LOG}</string>
  <key>StandardErrorPath</key>
  <string>${LOG}</string>
</dict>
</plist>
EOF
  uid=$(id -u)
  launchctl bootout "gui/${uid}/${LABEL}" 2>/dev/null || true
  launchctl bootstrap "gui/${uid}" "$PLIST"
  echo "installed ${PLIST}"
}

uninstall_agent() {
  uid=$(id -u)
  launchctl bootout "gui/${uid}/${LABEL}" 2>/dev/null || true
  rm -f "$PLIST"
  echo "removed ${LABEL}"
}

case "${1:-}" in
  --install) install_agent; exit 0 ;;
  --uninstall) uninstall_agent; exit 0 ;;
esac

# Not serving: nothing to stop.
lsof -nP -iTCP:"$port" -sTCP:LISTEN -t >/dev/null 2>&1 || exit 0

# A connected client means it is in use.
lsof -nP -iTCP:"$port" -sTCP:ESTABLISHED 2>/dev/null | awk 'NR>1' | grep -q . && exit 0

# No request has been served since the server log last changed.
log="${STATE}/server.log"
if [ -f "$log" ]; then
  idle=$(( $(date +%s) - $(date -r "$log" +%s) ))
  [ "$idle" -lt $(( idle_minutes * 60 )) ] && exit 0
fi

echo "$(date -u +%FT%TZ) idle ${idle_minutes}m: stopping bonsai on port ${port}"
BONSAI_PORT="$port" "$(dirname "$SCRIPT")/bonsai-stop.sh"
