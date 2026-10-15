#!/usr/bin/env bash
# Record the current tunnel URL for pi's colab-lifecycle extension, which points the
# `colab-qwen` provider at it. The API key lives in ~/.pi/agent/auth.json, never in Git.
# Usage: scripts/colab-qwen-connect.sh <https://xxx.trycloudflare.com>
set -euo pipefail

url="${1:?usage: $0 <tunnel-url>}"
[[ "$url" =~ ^https://[a-z0-9-]+\.trycloudflare\.com$ ]] || { echo "unexpected tunnel url: $url" >&2; exit 1; }

root="$(cd "$(dirname "$0")/.." && pwd)"
key="$(jq -er '.["google-colab"].key' "$root/.pi/agent/auth.json")"
state="${COLAB_QWEN_STATE:-$HOME/.cache/colab-qwen/state.json}"

curl -fsS --retry 24 --retry-delay 5 --retry-all-errors -m 30 -H "Authorization: Bearer $key" "$url/v1/models" >/dev/null
mkdir -p "$(dirname "$state")"
jq -nc --arg u "$url" --argjson t "$(($(date +%s) * 1000))" '{url:$u,lastRequestAt:$t}' >"$state"
echo "ok: $url"
