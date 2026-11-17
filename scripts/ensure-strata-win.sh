#!/usr/bin/env bash
# Ensure my-win's Strata is serving behind https://strata.kkk4oru.com: if Cloudflare returns 502
# (idle-stopped), start my-win's `StrataStart` scheduled task over SSH and wait until it answers.
#   STRATA_SSH_HOST  SSH alias (default: my-win)
#   STRATA_WIN_URL   endpoint base URL (default: https://strata.kkk4oru.com)
set -euo pipefail

host="${STRATA_SSH_HOST:-my-win}"
url="${STRATA_WIN_URL:-https://strata.kkk4oru.com}"
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
  echo "strata-win already serving at $url"
  exit 0
fi

if ! ssh -o BatchMode=yes -o ConnectTimeout=10 "$host" 'schtasks /run /tn StrataStart' >/dev/null 2>&1; then
  echo "strata-win: cannot run StrataStart on $host" >&2
  exit 1
fi

for _ in $(seq 1 60); do
  if serving; then
    echo "strata-win serving at $url"
    exit 0
  fi
  sleep 5
done

echo "strata-win: started on $host but $url never answered 200" >&2
exit 1
