#!/bin/bash
# Reclaim leaked Apple Pro app RegisterProExtension helpers.
# Every `Compressor -monitor -once` CLI call (and some Final Cut Pro launches) leaves one
# idle RegisterProExtension GUI process behind. Hundreds of them exhaust launchservicesd's
# dispatch threads; WindowServer then blocks on LaunchServices, keyboard/mouse input stops,
# and the userspace watchdog panics the Mac (seen 2026-10-16/18/19).
# Sends SIGTERM only (never SIGKILL) to this user's PPID-1 helpers older than MIN_AGE.

set -euo pipefail

THRESHOLD="${PRO_HELPER_THRESHOLD:-20}"
MIN_AGE="${PRO_HELPER_MIN_AGE:-120}"
LABEL="com.kkkaoru.pro-app-helper-reaper"
PLIST_PATH="${HOME}/Library/LaunchAgents/${LABEL}.plist"
DOTPATH=$(cd "$(dirname "$0")/.." || exit 1; pwd)
SCRIPT="${DOTPATH}/scripts/pro-app-helper-reaper.sh"
LOG_DIR="${HOME}/Library/Logs"
LOG="${LOG_DIR}/pro-app-helper-reaper.log"
HELPER_RE='^/Applications/[^/]+\.app/Contents/Helpers/RegisterProExtension\.app/Contents/MacOS/RegisterProExtension$'

# Prints "<pid> <age-seconds>" for every eligible helper.
helpers() {
  /bin/ps -axww -o pid=,ppid=,uid=,etime=,comm= | awk -v uid="$(id -u)" -v re="$HELPER_RE" '
    function seconds(t,   d, n, p) {
      d = 0
      if (index(t, "-")) { split(t, p, "-"); d = p[1]; t = p[2] }
      n = split(t, p, ":")
      return d * 86400 + (n == 3 ? p[1] * 3600 + p[2] * 60 + p[3] : p[1] * 60 + p[2])
    }
    {
      command = $0
      sub(/^ *[0-9]+ +[0-9]+ +-?[0-9]+ +[0-9:-]+ +/, "", command)
    }
    $2 == 1 && $3 == uid && command ~ re { print $1, seconds($4) }'
}

reap() {
  local rows count pid age signaled=0
  rows=$(helpers)
  count=$(printf '%s' "$rows" | grep -c . || true)
  if [ "$count" -lt "$THRESHOLD" ]; then
    return 0
  fi
  while read -r pid age; do
    if [ "$age" -ge "$MIN_AGE" ] && kill -TERM "$pid" 2>/dev/null; then
      signaled=$((signaled + 1))
    fi
  done <<< "$rows"
  echo "$(date '+%Y-%m-%dT%H:%M:%S%z') helpers=${count} sigterm=${signaled}"
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
  <key>RunAtLoad</key>
  <true/>
  <key>StartInterval</key>
  <integer>60</integer>
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
  --status) helpers | awk '{n++; if ($2 >= m) old++} END {printf "helpers=%d older_than_%ss=%d\n", n, m, old}' m="$MIN_AGE" ;;
  *) reap ;;
esac
