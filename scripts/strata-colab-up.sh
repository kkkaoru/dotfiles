#!/usr/bin/env bash
# Host Qwen3.8-Flash-Next on a Colab L4 with the Strata engine and point pi at it.
# Strata keeps the MoE experts in system RAM, so the host needs RAM, not VRAM: Q2_0 wants
# ~34 GB RAM + ~10 GB for the system, and an L4 (24 GB, Ada) is inside Strata's supported
# engine family where an A100 (SM 8.0) is not. Idempotent: it reuses a live session, engine,
# pack and tunnel. Stop it with: scripts/colab stop -s strata
set -euo pipefail
export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

root="$(cd "$(dirname "$0")/.." && pwd)"
colab_bin="${COLAB_BIN:-$root/scripts/colab}"
session="${STRATA_SESSION:-strata}"
gpu="${STRATA_GPU:-L4}"
port="${STRATA_PORT:-8081}"
state="${STRATA_STATE:-$HOME/.cache/strata/state.json}"
keyfile="$(dirname "$state")/key"
model_id="qwen3.8-flash-next-q2_0"

mkdir -p "$(dirname "$state")"
remote() { "$colab_bin" ssh -s "$session" -- "$@" 2>/dev/null; }

"$colab_bin" sessions 2>&1 | grep -q "\[$session\]" ||
  "$colab_bin" new -s "$session" --gpu "$gpu"

# The engine, the 66 GB pack and its preparation all resume from the previous run.
remote 'test -x "$HOME/.local/bin/strata" || (curl -fsSL --max-time 120 https://github.com/Niko1221/Strata/releases/latest/download/install.sh -o /tmp/si.sh && sh /tmp/si.sh)'

# Colab's own node service owns 8080, so the server runs beside it and cloudflared fronts it.
remote "cat > /content/strata-up.sh <<'EOS'
#!/bin/bash
export PATH=\"\$HOME/.local/bin:\$PATH\"
cd /content
exec \"\$HOME/.local/bin/strata\" launch --family qwen --model q2_0 --context 128k --vision off --drafting mtp --access local --port $port --no-tui --yes
EOS
chmod +x /content/strata-up.sh"
remote 'pgrep -f "[s]trata launch" >/dev/null || nohup bash /content/strata-up.sh >/content/strata-up.log 2>&1 </dev/null &'

# Wait until `strata launch` writes its key (up to 60s), then save it beside pi's credentials.
for _ in $(seq 1 30); do
  if remote '"$HOME/.local/bin/strata" launch --show-key' >"$keyfile.tmp" 2>/dev/null && [ -s "$keyfile.tmp" ]; then
    break
  fi
  sleep 2
done
[ -s "$keyfile.tmp" ] || { echo "strata did not produce an API key; see /content/strata-up.log in $session" >&2; exit 1; }
chmod 600 "$keyfile.tmp"
mv "$keyfile.tmp" "$keyfile"
key="$(cat "$keyfile")"

# Loading a 66 GB pack on 52 GB of RAM takes a while; an authenticated answer is the signal.
for _ in $(seq 1 60); do
  remote "curl -sf -m 3 -H 'Authorization: Bearer $key' http://127.0.0.1:$port/v1/models >/dev/null" && break
  sleep 20
done

remote "test -x /usr/local/bin/cloudflared || (curl -fsSL -o /usr/local/bin/cloudflared https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64 && chmod +x /usr/local/bin/cloudflared)"
remote 'pgrep -f "[c]loudflared tunnel" >/dev/null || nohup /usr/local/bin/cloudflared tunnel --no-autoupdate --url http://127.0.0.1:'"$port"' >/content/cf.log 2>&1 </dev/null &'

url=""
for _ in $(seq 1 15); do
  url="$(remote 'grep -aoE "https://[a-z0-9-]+\.trycloudflare\.com" /content/cf.log | head -1')"
  [ -n "$url" ] && break
  sleep 4
done
[ -n "$url" ] || { echo "no cloudflared tunnel URL yet; see /content/cf.log in $session" >&2; exit 1; }

curl -fsS --retry 12 --retry-delay 5 --retry-all-errors -m 30 \
  -H "Authorization: Bearer $key" "$url/v1/models" >/dev/null

jq -nc --arg u "$url" --arg m "$model_id" '{url:$u, model:$m}' >"$state"
auth="${PI_AUTH_JSON:-$root/.pi/agent/auth.json}"
[ -f "$auth" ] || echo '{}' >"$auth"
jq --arg k "$key" '.["strata-colab"] = {"type":"api_key","key":$k}' "$auth" >"$auth.tmp"
chmod 600 "$auth.tmp" && mv "$auth.tmp" "$auth"
echo "ok: $url  →  pi --model strata-colab/$model_id"
