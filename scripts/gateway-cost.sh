#!/usr/bin/env bash
# Print the AI Gateway's own cost for the last 24 hours, the number its dashboard shows.
#
# The account id, and a scoped token if the secret holds one, come from the encrypted
# gateway-cost secret (scripts/secrets.sh), so no credential sits in Git or in an argument.
# A scoped token with Account Analytics Read never expires and is used first. Wrangler's OAuth
# token is the fallback, and it expires hourly: a 401 asks wrangler once to renew it.
set -euo pipefail
# Herdr.app and other Dock-launched callers get no shell PATH, and age/op live in Homebrew.
PATH="/opt/homebrew/bin:/usr/local/bin:${PATH}"

repo=$(cd "$(dirname "$0")/.." && pwd)
secret=$(bash "$repo/scripts/secrets.sh" decrypt gateway-cost 2>/dev/null || true)
field() { printf '%s' "$secret" | plutil -extract "$1" raw -o - - 2>/dev/null || true; }

wrangler_config="$HOME/.wrangler/config/default.toml"
wrangler_token() {
  sed -n 's/^ *oauth_token *= *"\(.*\)"/\1/p' "$wrangler_config" 2>/dev/null | head -n 1
}

account="${CLOUDFLARE_ACCOUNT_ID:-$(field accountId)}"
token="${CLOUDFLARE_API_TOKEN:-$(field token)}"
[ -n "$token" ] || token=$(wrangler_token)
gateway=$(plutil -extract pi-gateway-cost.gateway raw -o - "$HOME/.pi/agent/settings.json" 2>/dev/null || true)
[ -n "$account" ] && [ -n "$token" ] && [ -n "$gateway" ] || exit 1

# Rolling last 24 hours in UTC, matching the Cloudflare AI Gateway dashboard's "Last 24 hours" view.
start=$(date -u -v-24H +%Y-%m-%dT%H:%M:%S.000Z)
end=$(date -u +%Y-%m-%dT%H:%M:%S.000Z)
query=$(printf '{ viewer { accounts(filter: { accountTag: "%s" }) { aiGatewayRequestsAdaptiveGroups(limit: 1, filter: { gateway: "%s", datetime_geq: "%s", datetime_leq: "%s" }) { count sum { cost } } } } }' \
  "$account" "$gateway" "$start" "$end")

# Prints the answer, then the HTTP status on the last line.
ask() {
  printf 'authorization: Bearer %s\n' "$1" |
    curl --silent --max-time 20 -H @- -H 'content-type: application/json' \
      --data "$(jq -nc --arg q "$query" '{query: $q}')" -w '\n%{http_code}' \
      https://api.cloudflare.com/client/v4/graphql
}

answer=$(ask "$token")
if [ "$(printf '%s' "$answer" | tail -n 1)" = 401 ] && command -v wrangler >/dev/null 2>&1; then
  wrangler whoami >/dev/null 2>&1 || true
  token=$(wrangler_token)
  answer=$(ask "$token")
fi

printf '%s' "$answer" | sed '$d' |
  jq -r '.data.viewer.accounts[0].aiGatewayRequestsAdaptiveGroups[0].sum.cost // empty'
