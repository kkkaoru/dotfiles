#!/usr/bin/env bash
# Ensure my-mac's Bonsai 2 27B is serving behind https://bonsai-mac.kkk4oru.com: if Cloudflare
# returns 502 (idle-stopped), start the MLX server there over SSH and wait until it answers.
#   BONSAI_SSH_HOST  SSH alias (default: my-mac)
#   BONSAI_MAC_URL   endpoint base URL (default: https://bonsai-mac.kkk4oru.com)
set -euo pipefail

host="${BONSAI_SSH_HOST:-my-mac}"
url="${BONSAI_MAC_URL:-https://bonsai-mac.kkk4oru.com}"
cf_dir="${BONSAI_CF_DIR:-$HOME/.config/strata}"

id="$(cat "$cf_dir/cf-client-id")"
sec="$(cat "$cf_dir/cf-client-secret")"

serving() {
  [ "$(curl -s -o /dev/null -m 5 -w '%{http_code}' \
    -H "CF-Access-Client-Id: $id" \
    -H "CF-Access-Client-Secret: $sec" \
    "$url/v1/models")" = "200" ]
}

if serving; then
  echo "bonsai-mac already serving at $url"
  exit 0
fi

# The remote script enables the prefix cache. Pass it here too, so an older copy still does.
if ! ssh -o BatchMode=yes -o ConnectTimeout=10 "$host" \
  'env APC_ENABLED=1 APC_DISK_ENABLED=1 APC_DEFAULT_TENANT=bonsai APC_MEMORY_MAX_GB=4 APC_DISK_MAX_GB=4 ~/.local/bin/bonsai-serve' \
  >/dev/null 2>&1; then
  echo "bonsai-mac: cannot run bonsai-serve on $host" >&2
  exit 1
fi

for _ in $(seq 1 30); do
  if serving; then
    echo "bonsai-mac serving at $url"
    exit 0
  fi
  sleep 1
done

echo "bonsai-mac: started on $host but $url never answered 200" >&2
exit 1
