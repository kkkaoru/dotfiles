#!/usr/bin/env bash
# Generate one image through the MiniMax-compatible endpoint and save it.
#   generate.sh "<prompt>" [out.png] [WIDTHxHEIGHT]
# Needs MINIMAX_API_BASE and MINIMAX_API_KEY (source ~/.config/colab-h3-image/env).
set -euo pipefail
prompt="${1:?usage: $0 <prompt> [out.png] [WIDTHxHEIGHT]}"
out="${2:-h3.png}"
size="${3:-2048x2048}"
: "${MINIMAX_API_BASE:?set MINIMAX_API_BASE}" "${MINIMAX_API_KEY:?set MINIMAX_API_KEY}"

body="$(jq -nc --arg p "$prompt" --arg s "$size" '{model:"MiniMax-H3-Image",prompt:$p,size:$s,seed:42,response_format:"b64_json"}')"
# Key goes via a header file on stdin-free curl config, so it never appears in `ps`.
curl -fsS --max-time 1800 -K - "${MINIMAX_API_BASE%/}/v1/images/generations" \
  -H 'Content-Type: application/json' -d "$body" <<<"header = \"Authorization: Bearer $MINIMAX_API_KEY\"" \
  | jq -er '.data[0].b64_json' | base64 -d >"$out"
file "$out"
