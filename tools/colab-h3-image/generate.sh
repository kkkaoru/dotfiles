#!/usr/bin/env bash
# Generate one image through the MiniMax-H3-Image endpoint and save it as PNG.
#   generate.sh "<prompt>" [out.png] [WIDTHxHEIGHT]
#   H3_SHARE=1 generate.sh ...        # also upload to R2 and print the public URL
# MINIMAX_API_KEY and MINIMAX_API_BASE come from the repository .env (loaded from any
# working directory); export either one to override it for a single run.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=/dev/null
. "$here/../../scripts/lib/dotfiles-env.sh"

prompt="${1:?usage: $0 <prompt> [out.png] [WIDTHxHEIGHT]}"
out="${2:-h3.png}"
size="${3:-2048x2048}"
: "${MINIMAX_API_BASE:?add MINIMAX_API_BASE to the dotfiles .env}" \
  "${MINIMAX_API_KEY:?add MINIMAX_API_KEY to the dotfiles .env}"

body="$(jq -nc --arg p "$prompt" --arg s "$size" '{model:"MiniMax-H3-Image",prompt:$p,size:$s,seed:42,response_format:"b64_json"}')"
# Key goes via a header file on stdin-free curl config, so it never appears in `ps`.
response="$(curl -fsS --max-time 1800 -K - "${MINIMAX_API_BASE%/}/v1/images/generations" \
  -H 'Content-Type: application/json' -d "$body" <<<"header = \"Authorization: Bearer $MINIMAX_API_KEY\"")"
# The server streams keep-alive spaces before the JSON; errors arrive as base_resp with HTTP 200.
if ! jq -e '.data[0].b64_json' >/dev/null <<<"$response"; then
  jq -c '.base_resp // .' <<<"$response" >&2
  exit 1
fi
jq -r '.data[0].b64_json' <<<"$response" | base64 -d >"$out"
file "$out"
[ "${H3_SHARE:-0}" = 1 ] && echo "url: $("$here/../../scripts/cf-share-image.sh" "$out")"
exit 0
