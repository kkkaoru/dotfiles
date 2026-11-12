#!/bin/bash
# Finish Herdr.app's setup after ./create-symlinks.sh has linked .config/herdr-app: build the
# terminal font settings.json names, then show the main-emoji layout in the open window.
# Idempotent: a step whose result is already in place is skipped, so running it again changes nothing.

set -euo pipefail

DOTPATH="$(cd "$(dirname "$0")/.." && pwd)"
SETTINGS="${HOME}/.config/herdr-app/settings.json"
LAYOUT="main-emoji"

if ! command -v herdr-app >/dev/null 2>&1; then
  echo "missing: herdr-app (install Herdr.app and open it once)" >&2
  exit 1
fi
if [ ! -L "$SETTINGS" ]; then
  echo "missing: ${SETTINGS} link (run ./create-symlinks.sh first)" >&2
  exit 1
fi

# "UDEV Gothic <weight> 35NFLG" -> <weight>; the build script names its files after it.
weight="$(sed -n 's/.*"font": *"UDEV Gothic \([A-Za-z]*\) 35NFLG".*/\1/p' "$SETTINGS")"
fonts=0
for style in Regular Bold Italic BoldItalic; do
  if [ -f "${HOME}/Library/Fonts/UDEVGothic${weight}35NFLG-${style}.ttf" ]; then
    fonts=$((fonts + 1))
  fi
done
if [ -z "$weight" ]; then
  echo "skipped font: settings.json does not name a UDEV Gothic <weight> 35NFLG font"
elif [ "$fonts" -eq 4 ]; then
  echo "present: UDEV Gothic ${weight} 35NFLG"
else
  "${DOTPATH}/scripts/build-udev-gothic-light.sh" "$weight"
fi

# The layout lives in the window's state, not in a file, so it needs the app running.
if ! status="$(herdr-app status 2>/dev/null)"; then
  status=""
fi
if printf '%s' "$status" | grep -q "\"layout\": \"${LAYOUT}\""; then
  echo "present: layout ${LAYOUT}"
elif [ -n "$status" ]; then
  herdr-app "do" layout.switch --name "$LAYOUT" >/dev/null
  echo "layout: ${LAYOUT}"
else
  echo "Herdr.app is not open: open it and run this again to switch to the ${LAYOUT} layout" >&2
fi

echo "ready: Herdr.app (restart it if the new font does not show)"
