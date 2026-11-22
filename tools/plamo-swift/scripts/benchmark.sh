#!/usr/bin/env bash
# Side-by-side numbers for the Swift and Python PLaMo servers: launch to ready, first and steady
# completion, resident memory, physical footprint and compressed memory.
#
#   scripts/benchmark.sh [swift|python|both]
#
# The Swift server needs the tokenizer blob, so it is dumped into a temp dir on first use.
set -euo pipefail
cd "$(dirname "$0")/.."

MODEL=${PLAMO_MODEL:-$HOME/.local/share/plamo-translate/plamo-3-translate-0.8b-mlx-4bit}
PYTHON=${PLAMO_PYTHON:-$HOME/.local/share/uv/tools/mlx-lm/bin/python}
SERVER=${PLAMO_SERVER:-$HOME/.local/bin/plamo-mlx-server}
PROMPT='<|plamo:tag|>input<|plamo:key|>lang<|plamo:val|>ja<|plamo:msg|>こんにちは、世界。<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>en<|plamo:msg|>'

# A nonce keeps the Python server's greedy-completion cache out of the measurement, and
# "cache": false covers servers that cache regardless.
body() {
  $PYTHON -c 'import json,sys; print(json.dumps({"prompt": sys.argv[1] + sys.argv[2], "max_tokens": 32, "temperature": 0, "cache": False}))' "$PROMPT" "$1"
}

now() { $PYTHON -c 'import time; print(f"{time.time():.3f}")'; }

# run <name> <port> <command...>
run() {
  local name=$1 port=$2
  shift 2
  "$@" >"/tmp/bench-$name.log" 2>&1 &
  local pid=$!
  local start ready first steady rss foot
  start=$(now)
  for _ in $(seq 1 1200); do
    curl -sf "http://127.0.0.1:$port/v1/models" >/dev/null 2>&1 && break
    sleep 0.1
  done
  ready=$($PYTHON -c "import time; print(f'{time.time()-$start:.2f}')")
  # Five distinct prompts: the first is cold, the rest give a median instead of one noisy sample.
  local times=() sample
  for i in 1 2 3 4 5; do
    sample=$(curl -s -o /dev/null -w '%{time_total}' -H 'Content-Type: application/json' \
      -d "$(body " nonce-$start-$i")" "http://127.0.0.1:$port/v1/completions")
    times+=("$sample")
  done
  first=${times[0]}
  steady=$($PYTHON -c 'import statistics,sys; print(f"{statistics.median(map(float, sys.argv[1:])):.3f}")' "${times[@]:1}")
  rss=$(ps -o rss= -p "$pid" | awk '{printf "%.0f MB", $1/1024}')
  foot=$(footprint -p "$pid" 2>/dev/null | grep -Eo 'phys_footprint: [0-9.]+ ?[KMG]B' | tr '\n' ' ')
  cmprs=$(top -l 1 -stats cmprs -pid "$pid" 2>/dev/null | tail -1 | awk '{print $1}')
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  printf '%-7s ready %ss  first %ss  steady %ss (median of 4)  rss %s  %s compressed %s\n' \
    "$name" "$ready" "$first" "$steady" "$rss" "${foot:-}" "${cmprs:-?}"
}

case ${1:-both} in
swift)
  blob=${TMPDIR:-/tmp}/plamo-tokenizer.bin
  [ -f "$blob" ] || $PYTHON scripts/dump-tokenizer.py "$MODEL" "$blob"
  run swift 18556 .build/release/plamo-swift serve --model "$MODEL" --tokenizer "$blob" \
    --port 18556 --idle-seconds 600
  ;;
python)
  run python 18557 "$SERVER" --model "$MODEL" --port 18557 --trust-remote-code
  ;;
both)
  "$0" python
  "$0" swift
  ;;
*)
  echo "usage: $0 [swift|python|both]" >&2
  exit 2
  ;;
esac
