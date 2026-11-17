#!/usr/bin/env bash
# Ensure my-mac's Strata is serving behind https://strata-mac.kkk4oru.com: if Cloudflare returns
# 502 (idle-stopped), run `strata-local` on my-mac over SSH and wait until the endpoint answers 200.
#   STRATA_MAC_SSH_HOST  SSH alias (default: my-mac)
#   STRATA_MAC_URL       endpoint base URL (default: https://strata-mac.kkk4oru.com)
set -euo pipefail

host="${STRATA_MAC_SSH_HOST:-my-mac}"
url="${STRATA_MAC_URL:-https://strata-mac.kkk4oru.com}"
cf_dir="${STRATA_CF_DIR:-$HOME/.config/strata}"

id="$(cat "$cf_dir/cf-client-id")"
sec="$(cat "$cf_dir/cf-client-secret")"

serving() {
  [ "$(curl -s -o /dev/null -m 5 -w '%{http_code}' \
    -H "CF-Access-Client-Id: $id" \
    -H "CF-Access-Client-Secret: $sec" \
    "$url/v1/models")" = "200" ]
}

if serving; then
  echo "strata-mac already serving at $url"
  exit 0
fi

if ! ssh -o BatchMode=yes -o ConnectTimeout=15 "$host" '~/.local/bin/strata-local' >/dev/null 2>&1; then
  echo "strata-mac: cannot run strata-local on $host" >&2
  exit 1
fi

for _ in $(seq 1 30); do
  if serving; then
    echo "strata-mac serving at $url"
    exit 0
  fi
  sleep 2
done

echo "strata-mac: started on $host but $url never answered 200" >&2
exit 1
