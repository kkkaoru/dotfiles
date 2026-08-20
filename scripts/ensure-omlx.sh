#!/bin/bash
# Start the managed oMLX server if it is not already healthy.

set -euo pipefail

OMLX_CLI="${OMLX_CLI:-/Applications/oMLX.app/Contents/MacOS/omlx-cli}"
PORT="${OMLX_PORT:-8891}"
HEALTH="http://127.0.0.1:${PORT}/health"

if curl -sf --max-time 2 "$HEALTH" >/dev/null; then
  exit 0
fi

if [ ! -x "$OMLX_CLI" ]; then
  echo "ensure-omlx: missing ${OMLX_CLI}" >&2
  exit 1
fi

"$OMLX_CLI" start --timeout 180
