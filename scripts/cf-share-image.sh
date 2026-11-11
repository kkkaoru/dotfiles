#!/usr/bin/env bash
# Upload a local image to the public R2 bucket and print its URL.
#   cf-share-image.sh <file> [content-type]
# The bucket comes from CF_SHARE_BUCKET in the repository .env, default "portfolio".
set -euo pipefail
# shellcheck source=/dev/null
. "$(cd "$(dirname "$0")/.." && pwd)/scripts/lib/dotfiles-env.sh"

bucket="${CF_SHARE_BUCKET:-portfolio}"
name="$(basename "$1" | tr -c 'A-Za-z0-9._-' '-' | sed 's/-*$//')"
key="share/$(date +%Y%m%d-%H%M%S)-$name"

cf r2 objects put "$key" --bucket-name "$bucket" --file "$1" \
  --content-type "${2:-image/png}" >/dev/null

domain="$(cf r2 buckets domains managed list --bucket-name "$bucket" | jq -r .domain)"
echo "https://$domain/$key"
