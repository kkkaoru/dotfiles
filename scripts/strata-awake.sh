#!/usr/bin/env bash
# Report whether each remote model endpoint is up, one line each: "<provider> awake|asleep|unknown".
# Read-only: it never starts or stops a server. pi's strata-lifecycle extension shows this in its
# footer; the idle-stop timers on each host are what make an idle endpoint go cold.
#   STRATA_CF_DIR   Cloudflare Access credentials (default: ~/.config/strata)
#   STRATA_STATE    strata-colab state with its quick-tunnel URL (default: ~/.cache/strata/state.json)
#   BONSAI_MAC_URL  my-mac's Bonsai endpoint (default: https://bonsai-mac.kkk4oru.com)
set -uo pipefail

cf_dir="${STRATA_CF_DIR:-$HOME/.config/strata}"
state="${STRATA_STATE:-$HOME/.cache/strata/state.json}"

# Any HTTP answer proves the tunnel and the server are up; Cloudflare Access answers 200 here.
probe() {
  local code
  code=$(curl -s -o /dev/null -m 8 -w '%{http_code}' "$@") || code=000
  case "$code" in
    000) echo asleep ;;
    5??) echo asleep ;;
    *) echo awake ;;
  esac
}

cloudflare() {
  local url="$1"
  if [ ! -r "$cf_dir/cf-client-id" ] || [ ! -r "$cf_dir/cf-client-secret" ]; then
    echo unknown
    return
  fi
  probe \
    -H "CF-Access-Client-Id: $(cat "$cf_dir/cf-client-id")" \
    -H "CF-Access-Client-Secret: $(cat "$cf_dir/cf-client-secret")" \
    "$url/v1/models"
}

echo "strata-win $(cloudflare "https://strata.kkk4oru.com")"
echo "strata-mac $(cloudflare "https://strata-mac.kkk4oru.com")"
echo "bonsai-mac $(cloudflare "${BONSAI_MAC_URL:-https://bonsai-mac.kkk4oru.com}")"

url="$(jq -r '.url // empty' "$state" 2>/dev/null)"
keyfile="$(dirname "$state")/key"
if [ -z "$url" ]; then
  echo "strata-colab unknown"
elif [ -r "$keyfile" ]; then
  echo "strata-colab $(probe -H "Authorization: Bearer $(cat "$keyfile")" "$url/v1/models")"
else
  echo "strata-colab $(probe "$url/v1/models")"
fi

exit 0
