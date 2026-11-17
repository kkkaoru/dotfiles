#!/usr/bin/env bash
# Ensure mac2's Bonsai 2 27B is served by oMLX behind https://bonsai-mac.kkk4oru.com.
# That hostname is on the ssh-mac2 tunnel, aimed at oMLX port 8891.
#   BONSAI_SSH_HOST  SSH alias (default: mac2)
#   BONSAI_MAC_URL   endpoint base URL (default: https://bonsai-mac.kkk4oru.com)
#   BONSAI_MODEL_ID  oMLX model id (default: Ternary-Bonsai-2-27B-mlx-2bit)
set -euo pipefail

host="${BONSAI_SSH_HOST:-mac2}"
url="${BONSAI_MAC_URL:-https://bonsai-mac.kkk4oru.com}"
model_id="${BONSAI_MODEL_ID:-Ternary-Bonsai-2-27B-mlx-2bit}"
cf_dir="${BONSAI_CF_DIR:-$HOME/.config/strata}"

id="$(cat "$cf_dir/cf-client-id")"
sec="$(cat "$cf_dir/cf-client-secret")"

serving() {
  curl -fsS -m 5 \
    -H "CF-Access-Client-Id: $id" \
    -H "CF-Access-Client-Secret: $sec" \
    "$url/v1/models" | grep -q "\"id\": \"$model_id\""
}

if serving; then
  echo "bonsai-mac already serving at $url"
  exit 0
fi

# Loading the server, then the weights, can take a couple of minutes.
if ! ssh -o BatchMode=yes -o ConnectTimeout=10 "$host" "\$HOME/.local/bin/ensure-omlx" >/dev/null 2>&1; then
  echo "bonsai-mac: cannot run ensure-omlx on $host" >&2
  exit 1
fi

for _ in $(seq 1 40); do
  if serving; then
    echo "bonsai-mac serving at $url"
    exit 0
  fi
  sleep 5
done

echo "bonsai-mac: oMLX started on $host but $url has no $model_id (tunnel must target port 8891)" >&2
exit 1
