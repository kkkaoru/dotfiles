#!/usr/bin/env bash
# Reach one machine's SSH server from anywhere through Cloudflare Tunnel + Access.
#   ./scripts/cf-remote-access.sh <label>           provision tunnel, Access app and DNS
#   ./scripts/cf-remote-access.sh <label> --agent   also keep the connector running (macOS)
# Run it once per machine: the Cloudflare side comes from here, then the printed
# connector command runs on the machine that owns the label.
# Env: CF_SSH_ZONE (kkk4oru.com), CF_SSH_SUBDOMAIN (ssh), CF_SSH_PORT (22),
#      CF_SSH_EMAIL (default: email of the logged-in Cloudflare user)
set -euo pipefail

label="${1:-}"
shift || true
agent=false
for arg in "$@"; do
  case "$arg" in
    --agent) agent=true ;;
    *) echo "unknown argument: $arg" >&2; exit 1 ;;
  esac
done
[[ "$label" =~ ^[a-z0-9]([a-z0-9-]*[a-z0-9])?$ ]] || {
  echo "usage: $0 <label> [--agent]   (label: lowercase letters, digits, dashes)" >&2
  exit 1
}

zone="${CF_SSH_ZONE:-kkk4oru.com}"
# First-level hostnames only: Universal SSL covers *.<zone>, not *.<label>.<zone>.
host="$label${CF_SSH_SUBDOMAIN:+.$CF_SSH_SUBDOMAIN}.$zone"
port="${CF_SSH_PORT:-22}"
tunnel_name="ssh-$label"
app_name="ssh $label"
email="${CF_SSH_EMAIL:-$(cf auth whoami | jq -r .email)}"
token_file="$HOME/.cloudflared/$tunnel_name.token"

# 1. Tunnel (remotely managed: the ingress below lives in Cloudflare, not on the host).
tunnel="$(cf tunnels list | jq -c --arg n "$tunnel_name" '[.[] | select(.name == $n)][0] // empty')"
if [[ -z "$tunnel" ]]; then
  tunnel="$(cf tunnels create --body "$(jq -nc --arg n "$tunnel_name" '{name: $n, config_src: "cloudflare"}')")"
  echo "created tunnel $tunnel_name"
fi
tunnel_id="$(jq -r .id <<<"$tunnel")"

cf tunnels config update "$tunnel_id" --body "$(jq -nc --arg h "$host" --arg s "ssh://127.0.0.1:$port" \
  '{config: {ingress: [{hostname: $h, service: $s}, {service: "http_status:404"}]}}')" >/dev/null

token="$(jq -r '.token // empty' <<<"$tunnel")"
if [[ -z "$token" ]]; then
  token="$(cf tunnels token get "$tunnel_id" | jq -r 'if type == "object" then (.token // .value) else . end')"
fi
umask 077
mkdir -p "$HOME/.cloudflared"
printf '%s\n' "$token" >"$token_file"

# 2. Access application + allow policy, created before DNS so the hostname is never open.
app_id="$(cf zero-trust access applications list | jq -r --arg n "$app_name" '[.[] | select(.name == $n)][0].id // empty')"
if [[ -z "$app_id" ]]; then
  app_id="$(cf zero-trust access applications create --body \
    "$(jq -nc --arg n "$app_name" --arg d "$host" '{name: $n, domain: $d, type: "ssh", session_duration: "24h"}')" |
    jq -r .id)"
  echo "created Access application $app_name"
else
  cf zero-trust access applications update "$app_id" --body \
    "$(jq -nc --arg n "$app_name" --arg d "$host" '{name: $n, domain: $d, type: "ssh"}')" >/dev/null
fi

cf zero-trust access applications policies list --app-id "$app_id" |
  jq -e --arg e "$email" 'map(select(.decision == "allow" and (.include // [] | any(.email.email == $e)))) | length > 0' \
    >/dev/null ||
  cf zero-trust access applications policies create "$app_id" --body "$(jq -nc --arg e "$email" \
    '{name: "owner", decision: "allow", include: [{email: {email: $e}}]}')" >/dev/null

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

# 4. Connector on this machine.
connector=("$(command -v cloudflared || echo cloudflared)" tunnel run --token-file "$token_file")
if $agent; then
  plist="$HOME/Library/LaunchAgents/com.kkkaoru.cloudflared-$tunnel_name.plist"
  service="com.kkkaoru.cloudflared-$tunnel_name"
  args=""
  for word in "${connector[@]}"; do
    args+="        <string>$word</string>"$'\n'
  done
  cat >"$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$service</string>
    <key>ProgramArguments</key>
    <array>
$args    </array>
    <key>KeepAlive</key>
    <true/>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardOutPath</key>
    <string>$HOME/Library/Logs/$tunnel_name.log</string>
    <key>StandardErrorPath</key>
    <string>$HOME/Library/Logs/$tunnel_name.log</string>
</dict>
</plist>
PLIST
  launchctl bootout "gui/$(id -u)/$service" 2>/dev/null || true
  sleep 1
  launchctl bootstrap "gui/$(id -u)" "$plist" 2>/dev/null ||
    launchctl kickstart -k "gui/$(id -u)/$service" ||
    echo "could not start $service; load it yourself: launchctl bootstrap gui/$(id -u) $plist"
fi

cat <<EOF

Provisioned $host  (tunnel $tunnel_name, token in $token_file)

On the machine that should be reachable, start the connector:
  ${connector[*]}
Keep it running after reboot there (macOS / Windows, needs sudo / admin):
  sudo bash -c 'cloudflared service install "\$(cat $token_file)"'

From any machine, log in through Access once, then SSH:
  cloudflared access ssh --hostname $host        # proxy on localhost
  cf access ssh-config --hostname $host          # ready-made ~/.ssh/config entry

Add the generated SSH block to ~/.ssh/config on the other machines to get
  ssh $label
EOF
