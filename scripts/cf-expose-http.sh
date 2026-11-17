#!/usr/bin/env bash
# Publish one machine's local HTTP service as https://<label>.<zone> through its existing
# Cloudflare Tunnel, behind an Access application that only a service token can reach.
# Idempotent: re-running it rewrites the same ingress entry, application and CNAME.
#   ./scripts/cf-expose-http.sh <label> <local-port> [tunnel-name]
# Env: CF_ZONE (kkk4oru.com), CF_SERVICE_TOKEN_NAME (pi-strata),
#      CF_SERVICE_TOKEN_CLIENT_ID (default: ~/.config/strata/cf-client-id),
#      CF_APP_EMAIL (default: the logged-in Cloudflare user's email, added as an allow rule)
set -euo pipefail

label="${1:-}"
port="${2:-}"
tunnel_name="${3:-ssh-$label}"
[[ "$label" =~ ^[a-z0-9]([a-z0-9-]*[a-z0-9])?$ ]] || {
  echo "usage: $0 <label> <local-port> [tunnel-name]   (label: lowercase letters, digits, dashes)" >&2
  exit 1
}
[[ "$port" =~ ^[0-9]+$ ]] || {
  echo "usage: $0 <label> <local-port> [tunnel-name]" >&2
  exit 1
}

zone="${CF_ZONE:-kkk4oru.com}"
host="$label.$zone"
service="http://127.0.0.1:$port"
token_name="${CF_SERVICE_TOKEN_NAME:-pi-strata}"
client_id="${CF_SERVICE_TOKEN_CLIENT_ID:-$(cat "$HOME/.config/strata/cf-client-id" 2>/dev/null || true)}"
email="${CF_APP_EMAIL:-$(cf auth whoami | jq -r .email)}"

# 1. The tunnel that already runs on the host takes one more hostname in its ingress.
tunnel="$(cf tunnels list | jq -c --arg n "$tunnel_name" '[.[] | select(.name == $n)][0] // empty')"
[[ -n "$tunnel" ]] || {
  echo "no tunnel named $tunnel_name" >&2
  exit 1
}
tunnel_id="$(jq -r .id <<<"$tunnel")"
ingress="$(cf tunnels config get "$tunnel_id" | jq -c --arg h "$host" --arg s "$service" '
  [.config.ingress[] | select(.hostname != $h and .service != "http_status:404")]
  + [{hostname: $h, service: $s, originRequest: {httpHostHeader: "127.0.0.1:" + ($s | split(":") | last)}}]
  + [{service: "http_status:404"}]')"
cf tunnels config update "$tunnel_id" --body "$(jq -nc --argjson i "$ingress" '{config: {ingress: $i}}')" >/dev/null
echo "tunnel $tunnel_name: $host -> $service"

# 2. Access application first, so the hostname is never briefly open. The service token keeps
#    pi's headers working; the owner's email is a fallback for a browser visit.
token_id="$(cf zero-trust access service-tokens list |
  jq -r --arg c "$client_id" --arg n "$token_name" \
    '[.[] | select(.client_id == $c or .name == $n)][0].id // empty')"
[[ -n "$token_id" ]] || {
  echo "no Access service token for client id $client_id" >&2
  exit 1
}
app_id="$(cf zero-trust access applications list |
  jq -r --arg d "$host" '[.[] | select(.domain == $d or ((.self_hosted_domains // []) | index($d)))][0].id // empty')"
# This CLI has no per-application policy commands, so both rules ride in the application body.
body="$(jq -nc --arg n "$label" --arg d "$host" --arg t "$token_id" --arg e "$email" \
  '{name: $n, domain: $d, type: "self_hosted", session_duration: "24h",
    policies: [
      {name: "pi service token", decision: "non_identity",
       include: [{service_token: {token_id: $t}}]},
      {name: "owner", decision: "allow", include: [{email: {email: $e}}]}
    ]}')"
if [[ -z "$app_id" ]]; then
  app_id="$(cf zero-trust access applications create --body "$body" | jq -r .id)"
  echo "created Access application $label"
else
  cf zero-trust access applications update "$app_id" --body "$body" >/dev/null
fi

# 3. Proxied CNAME pointing the hostname at the tunnel.
target="$tunnel_id.cfargotunnel.com"
record="$(cf dns records list --zone "$zone" --name-exact "$host" --type CNAME | jq -c '.[0] // empty')"
if [[ -z "$record" ]]; then
  cf dns records create --zone "$zone" --body "$(jq -nc --arg n "$host" --arg c "$target" \
    '{type: "CNAME", name: $n, content: $c, proxied: true}')" >/dev/null
  echo "created CNAME $host -> $target"
elif [[ "$(jq -r .content <<<"$record")" != "$target" ]]; then
  cf dns records update "$(jq -r .id <<<"$record")" --zone "$zone" --body "$(jq -nc \
    --arg n "$host" --arg c "$target" '{type: "CNAME", name: $n, content: $c, proxied: true}')" >/dev/null
  echo "updated CNAME $host -> $target"
fi

echo "published https://$host  (tunnel $tunnel_name, Access app $app_id)"
