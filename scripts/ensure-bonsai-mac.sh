#!/usr/bin/env bash
# Ensure Bonsai 2 27B is served by oMLX behind https://bonsai-mac.kkk4oru.com.
# That hostname is on the ssh-my-mac tunnel (this Mac), aimed at oMLX port 8891.
#   BONSAI_MAC_URL   endpoint base URL (default: https://bonsai-mac.kkk4oru.com)
#   BONSAI_MODEL_ID  oMLX model id (default: Ternary-Bonsai-2-27B-mlx-2bit)
set -euo pipefail

url="${BONSAI_MAC_URL:-https://bonsai-mac.kkk4oru.com}"
model_id="${BONSAI_MODEL_ID:-Ternary-Bonsai-2-27B-mlx-2bit}"
cf_dir="${BONSAI_CF_DIR:-$HOME/.config/strata}"

id="$(cat "$cf_dir/cf-client-id")"
sec="$(cat "$cf_dir/cf-client-secret")"

serving() {
  curl -fsS -m 5 \
    -H "CF-Access-Client-Id: $id" \
    -H "CF-Access-Client-Secret: $sec" \
    "$url/v1/models" | grep -F -q "$model_id"
}

if serving; then
  echo "bonsai-mac already serving at $url"
  exit 0
fi

# Loading the server, then the weights, can take a couple of minutes. The tunnel
# ends on this Mac, so start the local oMLX; SSH here would need an Access login.
if ! "$(dirname "$0")/ensure-omlx.sh" >/dev/null 2>&1 && ! ensure-omlx >/dev/null 2>&1; then
  echo "bonsai-mac: cannot start oMLX" >&2
  exit 1
fi

for _ in $(seq 1 40); do
  if serving; then
    echo "bonsai-mac serving at $url"
    exit 0
  fi
  sleep 5
done

echo "bonsai-mac: oMLX is up but $url does not list $model_id" >&2
exit 1
