#!/usr/bin/env bash
# Bring up MiniMax-H3-Image on a Colab A100 80GB (High-RAM) as a MiniMax-API-compatible endpoint.
#   tools/colab-h3-image/up.sh          # ~30-40 min on a fresh VM (135 GB of weights)
#   tools/colab-h3-image/generate.sh "a cat yawning" out.png
#   colab stop -s h3img                 # stop billing when done
# up.sh writes MINIMAX_API_BASE/MINIMAX_API_KEY into the repository .env (gitignored),
# which generate.sh reads from any working directory.
set -euo pipefail
export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

here="$(cd "$(dirname "$0")" && pwd)"
s=h3img
conf="$HOME/.config/colab-h3-image"
mkdir -p "$conf" && chmod 700 "$conf"
[ -s "$conf/api_key" ] || (umask 077 && openssl rand -hex 32 >"$conf/api_key")

# colab exec occasionally stops answering; a local hard timeout keeps the driver moving.
with_timeout() {
  local limit=$1
  shift
  if command -v gtimeout >/dev/null; then gtimeout -k 5 "$limit" "$@"; else "$@"; fi
}

run() { with_timeout "$((${2:-120} + 60))" colab exec -s "$s" --timeout "${2:-120}" -f "$1"; }
colab sessions 2>&1 | grep -q "\[$s\]" || colab new -s "$s" --gpu A100 --high-mem

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
# Ship the gateway and the key without putting the key on any command line.
python3 - "$here/gateway.py" "$conf/api_key" >"$work/push.py" <<'PY'
import sys
gateway, key = open(sys.argv[1]).read(), open(sys.argv[2]).read().strip()
print(f"""import os
os.makedirs('/content/h3', exist_ok=True)
open('/content/h3/gateway.py', 'w').write({gateway!r})
fd = os.open('/content/h3/api_key', os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
os.write(fd, {key!r}.encode()); os.close(fd)
print('pushed')""")
PY
run "$work/push.py" 60 >/dev/null

cat >"$work/ready.py" <<'PY'
import os
done = all(os.path.exists(f"/content/h3/{m}") for m in ("installed", "downloaded"))
print("READY" if done else "RUNNING" if os.path.exists("/content/h3/download.log") else "NONE")
PY
# Start install/download once; later runs only wait for the markers.
if run "$work/ready.py" 60 2>/dev/null | grep -q NONE; then run "$here/install.py" 60 >/dev/null; fi
# Wait for the install/download markers, but fail instead of polling forever (40 min budget).
for _ in $(seq 1 80); do
  run "$work/ready.py" 60 2>/dev/null | grep -q READY && break
  sleep 30
done
run "$work/ready.py" 120 2>/dev/null | grep -q READY || {
  echo "install/download did not finish; see /content/h3/{install,download}.log" >&2
  exit 1
}

url="$(run "$here/serve.py" 150 2>/dev/null | grep -oE 'https://[a-z0-9-]+\.trycloudflare\.com' | tail -1)"
[ -n "$url" ] || { echo "no tunnel url" >&2; exit 1; }
# Literal $(cat ...) on purpose: the env file reads the key when sourced instead of embedding it.
# shellcheck disable=SC2016
printf 'export MINIMAX_API_BASE=%q\nexport MINIMAX_API_KEY="$(cat %q)"\n' "$url" "$conf/api_key" >"$conf/env"
# Keep the repository .env current so scripts pick up the new tunnel URL from any directory.
repo_env="$(cd "$here/../.." && pwd)/.env"
touch "$repo_env"
sed -i '' -e '/^MINIMAX_API_BASE=/d' -e '/^MINIMAX_API_KEY=/d' "$repo_env"
{ echo "MINIMAX_API_BASE=$url"; echo "MINIMAX_API_KEY=$(cat "$conf/api_key")"; } >>"$repo_env"
echo "endpoint: $url (model loading; first request may take minutes)"
echo "use: tools/colab-h3-image/generate.sh 'a cat yawning' out.png"
