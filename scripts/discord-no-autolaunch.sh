#!/bin/sh
# macOS reopens the apps that were running at the last logout (loginwindow TAL),
# so Discord returns at login without being a login item. Quit it while it is
# still that relaunch, and the next snapshot no longer contains it.
for _ in 1 2 3 4 5 6 7 8 9 10 11 12; do
  pid=$(pgrep -x Discord | head -1)
  if [ -n "$pid" ]; then
    age=$(ps -o etime= -p "$pid" | awk -F'[-:]' '
      NF == 4 { print $1 * 86400 + $2 * 3600 + $3 * 60 + $4 }
      NF == 3 { print $1 * 3600 + $2 * 60 + $3 }
      NF == 2 { print $1 * 60 + $2 }')
    if [ "${age:-99999}" -le 120 ]; then
      kill -TERM "$pid"
      exit 0
    fi
    exit 0
  fi
  sleep 5
done
