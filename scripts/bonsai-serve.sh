#!/usr/bin/env bash
# Retired: pi serves Bonsai through oMLX (port 8891), not this mlx_vlm server.
# Left so an already-running process can still be identified. Do not start it for pi.
#   BONSAI_MODEL_DIR  MLX checkpoint (default: ~/Models/Ternary-Bonsai-2-27B-mlx-2bit)
#   BONSAI_VENV       python with the pinned mlx-vlm (default: ~/.local/share/bonsai-2-27b/venv)
#   BONSAI_PORT       port that must match .pi/agent/models.json (default: 8090)
set -euo pipefail

models="${BONSAI_MODEL_DIR:-$HOME/Models/Ternary-Bonsai-2-27B-mlx-2bit}"
venv="${BONSAI_VENV:-$HOME/.local/share/bonsai-2-27b/venv}"
port="${BONSAI_PORT:-8090}"
state="${HOME}/.local/state/bonsai"
log="${state}/server.log"
pidfile="${state}/server.pid"

[ -d "$models" ] || {
  echo "bonsai: no checkpoint at $models (run scripts/setup-bonsai.sh)" >&2
  exit 1
}

if nc -z 127.0.0.1 "$port" 2>/dev/null; then
  echo "bonsai already serving on 127.0.0.1:$port"
  exit 0
fi

mkdir -p "$state"
# Weight load is about a second. Re-prefilling an unchanged prompt is the slow part, so keep
# that prefix in memory while this process lives and on disk after idle-stop. Not a standby process.
export APC_ENABLED=1 APC_DISK_ENABLED=1 APC_DEFAULT_TENANT=bonsai
export APC_MEMORY_MAX_GB="${APC_MEMORY_MAX_GB:-4}" APC_DISK_MAX_GB="${APC_DISK_MAX_GB:-4}"
nohup "$venv/bin/python" -m mlx_vlm.server \
  --model "$models" --host 127.0.0.1 --port "$port" --enable-thinking \
  >>"$log" 2>&1 </dev/null &
echo $! >"$pidfile"

for _ in $(seq 1 90); do
  if curl -fsS -m 2 "http://127.0.0.1:$port/v1/models" >/dev/null 2>&1; then
    echo "bonsai serving on 127.0.0.1:$port"
    exit 0
  fi
  sleep 1
done

echo "bonsai: started but 127.0.0.1:$port never answered; see $log" >&2
exit 1
