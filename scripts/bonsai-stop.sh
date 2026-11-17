#!/usr/bin/env bash
# Stop the Bonsai MLX server this machine started (see bonsai-serve.sh).
#   BONSAI_PORT  port to drain (default: 8090)
#   BONSAI_WAIT  seconds to let an in-flight request finish (default: 20)
set -euo pipefail

port="${BONSAI_PORT:-8090}"
wait_seconds="${BONSAI_WAIT:-20}"
state="${HOME}/.local/state/bonsai"
pidfile="${state}/server.pid"

pid="$(cat "$pidfile" 2>/dev/null || true)"
if [ -z "$pid" ] || ! kill -0 "$pid" 2>/dev/null; then
  pid="$(lsof -nP -iTCP:"$port" -sTCP:LISTEN -t 2>/dev/null | head -1)"
fi
[ -n "$pid" ] || {
  echo "bonsai: nothing listening on $port"
  exit 0
}

kill "$pid"
for _ in $(seq 1 "$wait_seconds"); do
  kill -0 "$pid" 2>/dev/null || break
  sleep 1
done
kill -0 "$pid" 2>/dev/null && kill -9 "$pid" 2>/dev/null
rm -f "$pidfile"
echo "bonsai stopped"
