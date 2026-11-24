#!/usr/bin/env bash
# Serve Inkling 2 Mini (26B MoE, 5.4B active, NVFP4 experts) from a standalone llama.cpp build,
# so pi's `inkling-mini` provider has an OpenAI-compatible server on this Mac.
#   ./scripts/inkling-mini.sh start|stop|status|logs|idle-stop|install-idle|uninstall-idle
# The model is ~18 GB resident, so it is not a login item: pi's strata-lifecycle extension runs
# `start` when an inkling-mini model is selected, and `idle-stop` (launchd, every 2 min) stops it
# after INKLING_IDLE_MINUTES with no request and no client.
#   INKLING_PORT          port that must match .pi/agent/models.json (default: 8082)
#   INKLING_CTX           context window, the main memory knob (default: 32768)
#   INKLING_IDLE_MINUTES  idle window before idle-stop stops it (default: 10)
#   INKLING_MODEL_DIR     GGUF directory (default: ~/Models/Inkling-2-Mini-GGUF)
#   INKLING_LLAMA_SERVER  llama-server binary (default: ~/.local/opt/llama-inkling-mini/llama-server)
set -euo pipefail

PORT="${INKLING_PORT:-8082}"
CTX="${INKLING_CTX:-32768}"
IDLE_MINUTES="${INKLING_IDLE_MINUTES:-10}"
MODEL_DIR="${INKLING_MODEL_DIR:-$HOME/Models/Inkling-2-Mini-GGUF}"
LLAMA="${INKLING_LLAMA_SERVER:-$HOME/.local/opt/llama-inkling-mini/llama-server}"
MODEL="$MODEL_DIR/Inkling-2-Mini-UD-Q4_K_XL.gguf"
MMPROJ="$MODEL_DIR/mmproj-F16.gguf"
DRAFTER="$MODEL_DIR/dspark-Inkling-2-Mini-Q8_0.gguf"

STATE_DIR="$HOME/.local/state/inkling-mini"
PIDFILE="$STATE_DIR/pid"
LOG="$HOME/Library/Logs/inkling-mini.log"
LABEL="com.kkkaoru.inkling-mini-idle-stop"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
# The launchd job runs this through ~/.local/bin, so resolve the symlink to the repository copy.
SCRIPT=$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$0")

# /health answers 503 while the 18 GB of weights are still paging in, so only 200 means ready.
up() {
  [ "$(curl -s -o /dev/null -m 5 -w '%{http_code}' "http://127.0.0.1:$PORT/health")" = "200" ]
}

# `ollama run inkling-2-mini` would hold its own 20 GB copy; unload whatever Ollama has resident.
unload_ollama() {
  command -v ollama >/dev/null || return 0
  local loaded
  loaded=$(ollama ps 2>/dev/null | awk 'NR>1 {print $1}')
  [ -n "$loaded" ] || return 0
  echo "$loaded" | while read -r name; do
    ollama stop "$name" >/dev/null 2>&1 && echo "ollama: unloaded $name"
  done
}

start() {
  local missing=()
  for file in "$LLAMA" "$MODEL"; do [ -f "$file" ] || missing+=("$file"); done
  if [ "${#missing[@]}" -gt 0 ]; then
    echo "missing: ${missing[*]}" >&2
    echo "run ./scripts/setup-inkling-mini.sh first" >&2
    exit 1
  fi
  if up; then
    echo "inkling-2-mini already serving on 127.0.0.1:$PORT"
    return 0
  fi

  unload_ollama
  mkdir -p "$STATE_DIR"
  # A drafter or projector that is absent is not fatal: drop that flag and still serve the model.
  local args=(
    --model "$MODEL" --alias inkling-2-mini
    --ctx-size "$CTX" --parallel 1 --flash-attn on
    --n-gpu-layers 99
    --temp 1.0 --top-p 1.0 --top-k 0 --min-p 0.0
    --jinja --metrics
    --host 127.0.0.1 --port "$PORT"
  )
  [ -f "$MMPROJ" ] && args+=(--mmproj "$MMPROJ")
  if [ -f "$DRAFTER" ]; then
    args+=(--model-draft "$DRAFTER" --spec-type draft-dspark --spec-draft-n-max 2
      --n-gpu-layers-draft 99)
  fi

  echo "starting inkling-2-mini on 127.0.0.1:$PORT (ctx $CTX) ..."
  nohup "$LLAMA" "${args[@]}" >>"$LOG" 2>&1 &
  echo $! > "$PIDFILE"

  # 18 GB of weights take a while to page in on first start; 180 s is well past the observed load.
  local waited=0
  while [ "$waited" -lt 180 ]; do
    if up; then
      echo "ready: http://127.0.0.1:$PORT/v1"
      return 0
    fi
    if ! kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
      echo "llama-server exited; last lines of $LOG:" >&2
      tail -20 "$LOG" >&2
      exit 1
    fi
    sleep 3
    waited=$((waited + 3))
  done
  echo "still loading after ${waited}s; follow it with: $0 logs" >&2
  exit 1
}

stop() {
  local pids pid
  pids=$(lsof -nP -tiTCP:"$PORT" -sTCP:LISTEN 2>/dev/null || true)
  [ -n "$pids" ] || { echo "inkling-2-mini not serving on 127.0.0.1:$PORT"; return 0; }
  # SIGTERM lets llama-server free Metal buffers and the mmap cleanly.
  local pid_list=()
  while IFS= read -r pid; do pid_list+=("$pid"); done <<< "$pids" # no mapfile in bash 3.2
  kill "${pid_list[@]}" 2>/dev/null || true
  local waited=0
  while [ "$waited" -lt 20 ] && kill -0 "${pid_list[@]}" 2>/dev/null; do sleep 1; waited=$((waited + 1)); done
  kill -9 "${pid_list[@]}" 2>/dev/null || true
  rm -f "$PIDFILE"
  echo "stopped inkling-2-mini"
}

status() {
  if ! up; then
    echo "inkling-2-mini: stopped (port $PORT)"
    return 1
  fi
  local pid rss models
  pid=$(lsof -nP -tiTCP:"$PORT" -sTCP:LISTEN 2>/dev/null | head -1)
  rss=$(ps -o rss= -p "$pid" 2>/dev/null | awk '{printf "%.1f GB", $1/1048576}')
  models=$(curl -s -m 10 "http://127.0.0.1:$PORT/v1/models" | jq -r '[.data[].id] | join(", ")' 2>/dev/null)
  echo "inkling-2-mini: serving on 127.0.0.1:$PORT (pid $pid, RSS $rss)"
  echo "  models: ${models:-unknown}"
}

# llama-server writes a line per request, so the log's mtime is the last activity.
stop_if_idle() {
  up || exit 0
  lsof -nP -iTCP:"$PORT" -sTCP:ESTABLISHED 2>/dev/null | awk 'NR>1' | grep -q . && exit 0
  local last idle
  last=$(stat -f %m "$LOG" 2>/dev/null || echo 0)
  idle=$(( $(date +%s) - last ))
  [ "$idle" -ge $((IDLE_MINUTES * 60)) ] || exit 0
  stop
  echo "$(date '+%H:%M:%S') stopped after ${IDLE_MINUTES} min idle"
}

install_idle() {
  mkdir -p "${HOME}/Library/LaunchAgents" "$STATE_DIR"
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
    <string>idle-stop</string>
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
  <string>${STATE_DIR}/idle-stop.log</string>
  <key>StandardErrorPath</key>
  <string>${STATE_DIR}/idle-stop.log</string>
</dict>
</plist>
EOF
  uid=$(id -u)
  launchctl bootout "gui/${uid}/${LABEL}" 2>/dev/null || true
  launchctl bootstrap "gui/${uid}" "$PLIST"
  echo "installed ${PLIST}"
}

uninstall_idle() {
  uid=$(id -u)
  launchctl bootout "gui/${uid}/${LABEL}" 2>/dev/null || true
  rm -f "$PLIST"
  echo "removed ${LABEL}"
}

case "${1:-}" in
  start) start ;;
  stop) stop ;;
  status) status ;;
  logs) tail -f "$LOG" ;;
  idle-stop) stop_if_idle ;;
  install-idle) install_idle ;;
  uninstall-idle) uninstall_idle ;;
  *)
    echo "usage: $0 start|stop|status|logs|idle-stop|install-idle|uninstall-idle" >&2
    exit 1
    ;;
esac
