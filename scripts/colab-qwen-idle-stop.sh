#!/usr/bin/env bash
# Detached watcher: stop the Colab `qwen` session after 10 minutes without a colab-qwen request.
# pi's colab-lifecycle extension spawns it; the lock makes extra spawns exit immediately.
set -euo pipefail
export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

state="${COLAB_QWEN_STATE:-$HOME/.cache/colab-qwen/state.json}"
idle_ms="${COLAB_QWEN_IDLE_MS:-600000}"
poll="${COLAB_QWEN_POLL_SECONDS:-30}"
colab_bin="${COLAB_BIN:-$HOME/.local/bin/colab}"
lock="$(dirname "$state")/watcher.lock"

mkdir -p "$(dirname "$state")"
if ! mkdir "$lock" 2>/dev/null; then
  pid="$(cat "$lock/pid" 2>/dev/null || true)"
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    exit 0
  fi
  rm -rf "$lock"
  mkdir "$lock"
fi
echo $$ >"$lock/pid"
trap 'rm -rf "$lock"' EXIT

while sleep "$poll"; do
  url="$(jq -r '.url // empty' "$state" 2>/dev/null || true)"
  [ -n "$url" ] || exit 0
  last="$(jq -r '.lastRequestAt // 0' "$state")"
  now=$(($(date +%s) * 1000))
  if [ $((now - last)) -ge "$idle_ms" ]; then
    echo "$(date -u +%FT%TZ) idle $((now - last))ms: stopping colab session qwen"
    jq -c 'del(.url)' "$state" >"$state.tmp" && mv "$state.tmp" "$state"
    "$colab_bin" stop -s qwen || true
    exit 0
  fi
done
