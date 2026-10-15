#!/usr/bin/env bash
# One-time setup of the google-colab (Qwen3.8-27B) provider on this machine. Idempotent.
set -euo pipefail
export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

root="$(cd "$(dirname "$0")/.." && pwd)"

command -v jq >/dev/null || { echo "missing: jq (brew install jq)" >&2; exit 1; }
command -v bun >/dev/null || { echo "missing: bun (https://bun.sh)" >&2; exit 1; }

if ! command -v colab >/dev/null; then
  command -v uv >/dev/null || { echo "missing: uv (brew install uv)" >&2; exit 1; }
  uv tool install google-colab-cli
fi

(cd "$root/tools/pi-colab-lifecycle-extension" && bun install --frozen-lockfile)

mkdir -p "$root/.pi/agent/extensions"
ln -sfn "$root/tools/pi-colab-lifecycle-extension" "$root/.pi/agent/extensions/colab-lifecycle"

"$root/scripts/colab-qwen-ensure-key.sh"

if ! colab sessions >/dev/null 2>&1; then
  echo "colab is not authenticated yet. Run once (needs a browser):" >&2
  echo "  colab --auth=oauth2 sessions   # or: gcloud auth application-default login (see \`colab skill\`)" >&2
  exit 1
fi
echo "ok: pick google-colab / Qwen3.8-27B in pi (first use starts a Colab A100, ~10 min)"
