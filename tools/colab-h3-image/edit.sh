#!/usr/bin/env bash
# Edit 1-16 reference images with MiniMax-H3-Image (MiniMax `POST /v1/images/edits`).
#   edit.sh "<prompt>" ref.png [ref2.png ...]
#   OUT=out.png SIZE=4096x4096 edit.sh ...      # SIZE may be "auto" with ASPECT_RATIO / AREA
#   H3_SHARE=1 edit.sh ...                      # also upload the result to R2
# MINIMAX_API_KEY and MINIMAX_API_BASE come from the repository .env (any working directory).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=/dev/null
. "$here/../../scripts/lib/dotfiles-env.sh"

prompt="${1:?usage: $0 <prompt> <reference image> [more images...]}"
shift
[ "$#" -ge 1 ] || { echo "need at least one reference image" >&2; exit 1; }
out="${OUT:-edit.png}"
: "${MINIMAX_API_BASE:?add MINIMAX_API_BASE to the dotfiles .env}" \
  "${MINIMAX_API_KEY:?add MINIMAX_API_KEY to the dotfiles .env}"

# Reference images are large, so every payload goes through files: base64 or a data URI on a
# command line exceeds ARG_MAX.
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
count=0
for file in "$@"; do
  count=$((count + 1))
  { printf 'data:%s;base64,' "$(file --brief --mime-type "$file")"; base64 <"$file" | tr -d '\n'; } \
    >"$work/$count.uri"
done

jq -n --arg p "$prompt" --arg s "${SIZE:-2048x2048}" --arg ar "${ASPECT_RATIO:-}" --arg area "${AREA:-}" '
  {model: "MiniMax-H3-Image", prompt: $p, seed: 42, response_format: "b64_json", images: []}
  | if $s == "auto" then
      (if $ar != "" then .aspect_ratio = $ar else . end) | (if $area != "" then .area = $area else . end)
    else .size = $s end' >"$work/body.json"
for n in $(seq 1 "$count"); do
  jq --rawfile url "$work/$n.uri" '.images += [{image_url: $url}]' "$work/body.json" >"$work/next.json"
  mv "$work/next.json" "$work/body.json"
done

curl -fsS --max-time 1800 -K - "${MINIMAX_API_BASE%/}/v1/images/edits" -H 'Content-Type: application/json' \
  --data-binary @"$work/body.json" <<<"header = \"Authorization: Bearer $MINIMAX_API_KEY\"" >"$work/response.json"
if ! jq -e '.data[0].b64_json' >/dev/null "$work/response.json"; then
  jq -c '.error // .base_resp // .' "$work/response.json" >&2
  exit 1
fi
jq -r '.data[0].b64_json' "$work/response.json" | base64 -d >"$out"
file "$out"
[ "${H3_SHARE:-0}" = 1 ] && echo "url: $("$here/../../scripts/cf-share-image.sh" "$out")"
exit 0
