#!/bin/bash
#
# Record one-line Thunderbolt/USB4 snapshots for the OWC Thunderbolt 5 Hub setup, so the Mac can be
# used for real work and still leave evidence: a panic shows up as a new boot plus a higher panic
# count, a link downgrade is visible as the speed/width set changing, and `flaps` counts the hub's
# upstream-link teardowns in the last interval - the bursts that precede the ACIO0 assert
# (assert_id=2548).
#
# This cannot prevent that assert. It only records and warns.
# Cost: one snapshot is ~0.1-0.2 s of CPU (two ioreg walks, more when the hub is attached) plus
# ~1.2 s for the log query behind `flaps`; the agent runs it every 5 minutes at background priority
# (ProcessType Background, LowPriorityIO, Nice 10), about 0.3% of one core.
#
#   ./scripts/tb-watch.sh            # append one snapshot (the launchd agent runs this)
#   ./scripts/tb-watch.sh show       # last 20 snapshots
#   ./scripts/tb-watch.sh tail       # follow the log
#   ./scripts/tb-watch.sh install    # start the 5 min launchd agent
#   ./scripts/tb-watch.sh uninstall

set -euo pipefail

if [ "$(uname -s)" != "Darwin" ]; then
  echo "this script is macOS only" >&2
  exit 1
fi

SCRIPT_PATH="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"
LABEL="com.kkkaoru.tb-watch"
LOG="${HOME}/Library/Logs/tb-watch.log"
PLIST="${HOME}/Library/LaunchAgents/${LABEL}.plist"
INTERVAL=300
HUB_CLASS="IOThunderboltSwitchIntelJHL9580"
PANIC_DIR="/Library/Logs/DiagnosticReports"

# Distinct non-zero values of a numeric ioreg field across all Thunderbolt ports, sorted.
# 0 means no link on that port and only adds noise.
# Both link fields come from one ioreg walk: each walk costs ~20 ms of kernel time.
port_fields() {
  local raw
  raw=$(ioreg -w0 -c IOThunderboltPort -r -d 0 2>/dev/null | awk -F'= ' '
    /"Current Link Speed"/ { s = s " " $2 }
    /"Current Link Width"/ { w = w " " $2 }
    END { print s "|" w }')
  speed=$(distinct "${raw%%|*}")
  width=$(distinct "${raw##*|}")
}

distinct() {
  local values
  # The link lists arrive space-separated, one field per port, and 0 means no link on that port and
  # only adds noise, so split them here rather than leaving a caller's expansion unquoted.
  values=$(printf '%s\n' "$@" | tr ' ' '\n' | grep -vx 0 | sort -un | paste -sd, - || true)
  printf '%s' "${values:-none}"
}

snapshot() {
  local hub_out hub rom bootuuid bootsec up panics last speed width flaps
  hub_out=$(ioreg -w0 -c "$HUB_CLASS" -r -d 1 2>/dev/null || true)
  if [ -n "$hub_out" ]; then
    hub=yes
    rom=$(printf '%s\n' "$hub_out" | awk -F'= ' '/"ROM Version"/{print $2; exit}')
    rom=${rom:-?}
  else
    hub=no
    rom=-
  fi

  bootuuid=$(sysctl -n kern.bootuuid | cut -c1-8)
  bootsec=$(sysctl -n kern.boottime | sed -E 's/^\{ sec = ([0-9]+),.*/\1/')
  up="$(( ($(date +%s) - bootsec) / 60 ))m"
  panics_list=$(ls "${PANIC_DIR}"/panic-full-*.panic 2>/dev/null || true)
  panics=$(printf '%s' "$panics_list" | grep -c . || true)
  last=$(printf '%s\n' "$panics_list" | tail -1 | sed -E 's#.*panic-full-(.*)\.[0-9]+\.panic#\1#')
  port_fields
  # One line per hub teardown; a burst of these is what the ACIO0 assert follows.
  flaps=$(log show --last "$((INTERVAL / 60))m" --style compact \
    --predicate 'eventMessage CONTAINS "finalize - terminate device"' 2>/dev/null |
    grep -c 'finalize - terminate device' || true)

  printf '%s boot=%s up=%s hub=%s rom=%s speed=%s width=%s flaps=%s panics=%s last=%s\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$bootuuid" "$up" "$hub" "$rom" \
    "$speed" "$width" "${flaps:-?}" "$panics" "${last:-none}"
}

case "${1:-log}" in
  log)
    mkdir -p "$(dirname "$LOG")"
    if [ -f "$LOG" ] && [ "$(wc -c <"$LOG")" -gt 2097152 ]; then
      mv -f "$LOG" "${LOG}.1"
    fi
    snapshot >>"$LOG"
    ;;
  show)
    tail -n 20 "$LOG"
    ;;
  tail)
    tail -f "$LOG"
    ;;
  install)
    mkdir -p "$(dirname "$PLIST")"
    cat >"$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>${LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>${SCRIPT_PATH}</string>
    <string>log</string>
  </array>
  <key>StartInterval</key><integer>${INTERVAL}</integer>
  <key>RunAtLoad</key><true/>
  <key>ProcessType</key><string>Background</string>
  <key>LowPriorityIO</key><true/>
  <key>Nice</key><integer>10</integer>
  <key>StandardErrorPath</key><string>${HOME}/Library/Logs/tb-watch.err</string>
</dict>
</plist>
EOF
    launchctl bootout "gui/$(id -u)/${LABEL}" 2>/dev/null || true
    launchctl bootstrap "gui/$(id -u)" "$PLIST"
    echo "installed ${LABEL} (every ${INTERVAL}s) -> ${LOG}"
    ;;
  uninstall)
    launchctl bootout "gui/$(id -u)/${LABEL}" 2>/dev/null || true
    rm -f "$PLIST"
    echo "removed ${LABEL}"
    ;;
  *)
    echo "usage: $0 [log|show|tail|install|uninstall]" >&2
    exit 2
    ;;
esac
