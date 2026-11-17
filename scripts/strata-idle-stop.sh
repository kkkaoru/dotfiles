#!/usr/bin/env bash
# Stop this Mac's Strata after STRATA_IDLE_MINUTES with no request and no client connection.
# A managed instance holds ~20 GB of RAM and streams a 37.6 GB shard, so it must not idle forever.
#   --install / --uninstall   the launchd agent that runs this check every 2 minutes
#   STRATA_PROFILE            saved profile name (default: launch-q2_0-32k)
#   STRATA_LOCAL_PORT         port that must match .pi/agent/models.json (default: 8080)
#   STRATA_IDLE_MINUTES       idle window before stopping (default: 10)
set -euo pipefail

LABEL="com.kkkaoru.strata-idle-stop"
PLIST="${HOME}/Library/LaunchAgents/${LABEL}.plist"
# The launchd job runs this through ~/.local/bin, so resolve the symlink to the repository copy.
SCRIPT=$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$0")
LOG_DIR="${HOME}/.local/state/strata"
LOG="${LOG_DIR}/idle-stop.log"

profile="${STRATA_PROFILE:-launch-q2_0-32k}"
port="${STRATA_LOCAL_PORT:-8080}"
idle_minutes="${STRATA_IDLE_MINUTES:-10}"

install_agent() {
  mkdir -p "${HOME}/Library/LaunchAgents" "$LOG_DIR"
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
  launchctl bootstrap "gui/${uid}" "$PLIST"
  echo "installed ${PLIST}"
}

uninstall_agent() {
  uid=$(id -u)
  launchctl bootout "gui/${uid}/${LABEL}" 2>/dev/null || true
  rm -f "$PLIST"
  echo "removed ${LABEL}"
}

has_clients() {
  lsof -nP -iTCP:"${port}" -sTCP:ESTABLISHED 2>/dev/null | awk 'NR>1' | grep -q .
}

# The newest timestamp in `strata logs` is the last request activity (HH:MM:SS, today).
idle_seconds() {
  local last
  last=$(strata logs 2>/dev/null | grep -oE '[0-9]{2}:[0-9]{2}:[0-9]{2}' | tail -1)
  [ -n "$last" ] || { echo 0; return; }
  local now
  now=$(date +%s)
  local last_ts
  last_ts=$(date -j -f "%H:%M:%S" "$last" +%s 2>/dev/null || echo "$now")
  # A timestamp ahead of now means the log straddles midnight: treat it as active.
  if [ "$last_ts" -gt "$now" ]; then echo 0; else echo $((now - last_ts)); fi
}

stop_if_idle() {
  # Any HTTP answer (even 401) proves the server is up; 000 means it is already stopped.
  [ "$(curl -s -o /dev/null -m 5 -w '%{http_code}' "http://127.0.0.1:${port}/v1/models")" != "000" ] || exit 0
  has_clients && exit 0
  [ "$(idle_seconds)" -ge $((idle_minutes * 60)) ] || exit 0
  strata runtime stop --profile "$profile"
  echo "$(date '+%H:%M:%S') stopped after ${idle_minutes} min idle"
}

case "${1:-}" in
  --install) install_agent ;;
  --uninstall) uninstall_agent ;;
  *) stop_if_idle ;;
esac
