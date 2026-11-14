#!/usr/bin/env bash
# Generate one video (with audio) through the Pika H3 Flex endpoint and save it as MP4 + JSON metadata.
#   generate.sh [options] "<prompt>" [out.mp4]
#     -f first.png   first frame (i2va; with -l, fl2va)     -l last.png   last frame (l2va)
#     -r file        ordered reference image/video/audio (repeatable; ref2va: <=9 images, <=3 videos,
#                    <=3 audio clips, videos/audio 2-15 s)
#     -R 480p|768p   (default 480p)  -a 16:9 ...  -d 5..15 seconds  -s seed (default 42)
#     -O             CPU offload from the start (otherwise retried with offload only after a GPU OOM)
#     -k             keep the VM running afterwards (by default `colab stop -s pika` runs as soon as this
#                    job is done and no other job is queued or running; up.sh then takes ~30-40 min)
# The task follows the inputs: none -> t2va. Files go straight from this Mac to the Colab VM.
# PIKA_API_BASE / PIKA_API_KEY come from the repository .env written by up.sh.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=/dev/null
. "$here/../../scripts/lib/dotfiles-env.sh"

form=()
keep=0
while getopts "f:l:r:R:a:d:s:Ok" opt; do
  case "$opt" in
    f) form+=(-F "first_image=@$OPTARG") ;;
    l) form+=(-F "last_image=@$OPTARG") ;;
    r) form+=(-F "ref=@$OPTARG") ;;
    R) form+=(-F "resolution=$OPTARG") ;;
    a) form+=(-F "aspect=$OPTARG") ;;
    d) form+=(-F "duration=$OPTARG") ;;
    s) form+=(-F "seed=$OPTARG") ;;
    O) form+=(-F "cpu_offload=1") ;;
    k) keep=1 ;;
    *) sed -n '2,12p' "$0" >&2; exit 2 ;;
  esac
done
shift $((OPTIND - 1))
prompt="${1:?usage: $0 [options] <prompt> [out.mp4]}"
out="${2:-pika.mp4}"
[ ! -e "$out" ] || { echo "$out already exists" >&2; exit 1; }
: "${PIKA_API_BASE:?run tools/colab-pika-h3-flex/up.sh first}" "${PIKA_API_KEY:?run tools/colab-pika-h3-flex/up.sh first}"
base="${PIKA_API_BASE%/}/v1/videos"

# The key goes through a curl config on stdin, so it never appears in `ps`.
api() { curl -fsS -K - "$@" <<<"header = \"Authorization: Bearer $PIKA_API_KEY\""; }
# `prompt=<-` sends the prompt from stdin, so a leading @ or < in it is not read as a file; the key
# then goes through a /dev/fd config instead.
job="$(printf '%s' "$prompt" | curl -sS --max-time 600 -K <(printf 'header = "Authorization: Bearer %s"\n' "$PIKA_API_KEY") \
  -F "prompt=<-" "${form[@]}" "$base")"
# A gone VM answers with Cloudflare's HTML error page; show the JSON error, else a hint.
id="$(jq -er '.id' <<<"$job" 2>/dev/null)" || {
  jq -er '.error' <<<"$job" 2>/dev/null >&2 || echo "no Pika endpoint at $PIKA_API_BASE; run tools/colab-pika-h3-flex/up.sh" >&2
  exit 1
}
echo "job $id ($(jq -r .task <<<"$job")) queued"

# Stop billing the moment the last job is done (succeeded or failed), not while another one waits.
# shortcut: a VM whose last client was killed mid-wait keeps running; stop it with `colab stop -s pika`.
stop_when_idle() {
  [ "$keep" = 0 ] || return 0
  [ "$(api --max-time 60 "$base" | jq -r .active)" = 0 ] || { echo "other jobs still running; VM kept" >&2; return 0; }
  colab stop -s pika >/dev/null 2>&1 && echo "no jobs left; stopped the Colab VM (colab stop -s pika)" >&2
}
trap stop_when_idle EXIT

while :; do
  sleep 15
  state="$(api --max-time 60 "$base/$id")" || continue  # the tunnel can drop a poll; try again
  case "$(jq -r .status <<<"$state")" in
    succeeded) break ;;
    failed) jq -r .error <<<"$state" >&2; exit 1 ;;
  esac
done
api --max-time 600 -o "$out" "$base/$id/content"
api --max-time 60 -o "${out%.*}.json" "$base/$id/metadata"
echo "saved $out and ${out%.*}.json"
