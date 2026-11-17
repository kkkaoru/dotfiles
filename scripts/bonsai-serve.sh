#!/usr/bin/env bash
# Start Bonsai 2 27B (Ternary, MLX 2-bit) where pi's bonsai model expects it (127.0.0.1:8090).
# Nothing starts it in advance: pi's lifecycle extension runs this when a Bonsai model is
# selected, and bonsai-idle-stop stops it again after BONSAI_IDLE_MINUTES.
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
# The server loads 8.6 GB of weights before it answers, so detach it and wait on the API.
nohup "$venv/bin/python" -m mlx_vlm.server \
  --model "$models" --host 127.0.0.1 --port "$port" --enable-thinking \
  >>"$log" 2>&1 </dev/null &
echo $! >"$pidfile"

for _ in $(seq 1 60); do
  if curl -fsS -m 3 "http://127.0.0.1:$port/v1/models" >/dev/null 2>&1; then
    echo "bonsai serving on 127.0.0.1:$port"
    exit 0
  fi
  sleep 3
done

echo "bonsai: started but 127.0.0.1:$port never answered; see $log" >&2
exit 1
