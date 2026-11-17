#!/bin/bash
# Start the managed oMLX server if it is not already healthy.

set -euo pipefail

OMLX_CLI="${OMLX_CLI:-/Applications/oMLX.app/Contents/MacOS/omlx-cli}"
PORT="${OMLX_PORT:-8891}"
HEALTH="http://127.0.0.1:${PORT}/health"
BONSAI_ID="${BONSAI_MODEL_ID:-Ternary-Bonsai-2-27B-mlx-2bit}"
BONSAI_SRC="${BONSAI_MODEL_DIR:-$HOME/Models/Ternary-Bonsai-2-27B-mlx-2bit}"
BONSAI_LINK="$HOME/.omlx/models/$BONSAI_ID"

# oMLX only scans model dirs at start. Link the checkpoint first, or a healthy
# server that started earlier answers 404 for every Bonsai request.
if [ -f "$BONSAI_SRC/config.json" ]; then
  mkdir -p "$HOME/.omlx/models"
  if [ ! -e "$BONSAI_LINK" ] || [ -L "$BONSAI_LINK" ]; then
    ln -sfn "$BONSAI_SRC" "$BONSAI_LINK"
  fi
fi

serving_bonsai() {
  curl -sf --max-time 2 -H "Authorization: Bearer ${OMLX_API_KEY:-omlx}" \
    "http://127.0.0.1:${PORT}/v1/models" | grep -F -q "$BONSAI_ID"
}

if curl -sf --max-time 2 "$HEALTH" >/dev/null; then
  if [ ! -f "$BONSAI_SRC/config.json" ] || serving_bonsai; then
    exit 0
  fi
  "$OMLX_CLI" restart --timeout 180
  exit 0
fi

if [ ! -x "$OMLX_CLI" ]; then
  echo "ensure-omlx: missing ${OMLX_CLI}" >&2
  exit 1
fi

"$OMLX_CLI" start --timeout 180
