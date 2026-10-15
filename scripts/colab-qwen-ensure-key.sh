#!/usr/bin/env bash
# Make sure pi's auth.json has an API key for the google-colab provider; create one if missing.
# The key stays in ~/.pi/agent/auth.json (never in Git) and is reused for every Colab session.
set -euo pipefail
export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

auth="${PI_AUTH_JSON:-$(cd "$(dirname "$0")/.." && pwd)/.pi/agent/auth.json}"

if [ -f "$auth" ] && jq -e '.["google-colab"].key' "$auth" >/dev/null 2>&1; then
  exit 0
fi

mkdir -p "$(dirname "$auth")"
[ -f "$auth" ] || echo '{}' >"$auth"
key="$(openssl rand -base64 36 | tr '+/' '-_' | tr -d '=\n')"
tmp="$(mktemp "$auth.XXXXXX")"
jq --arg k "$key" '.["google-colab"] = {"type":"api_key","key":$k}' "$auth" >"$tmp"
chmod 600 "$tmp"
mv "$tmp" "$auth"
echo "created google-colab api key in $auth" >&2
