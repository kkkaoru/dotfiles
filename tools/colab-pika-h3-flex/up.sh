#!/usr/bin/env bash
# Bring up Pika H3 Flex on a Colab GPU (H100, else A100 80GB High-RAM) behind a job API.
#   tools/colab-pika-h3-flex/up.sh        # ~15-20 min on a fresh VM (~78 GB shared weights), ~1 min warm
#   tools/colab-pika-h3-flex/generate.sh "a cat yawning" cat.mp4
#   colab stop -s pika                    # stop billing when done
# up.sh writes PIKA_API_BASE/PIKA_API_KEY into the repository .env (gitignored), which generate.sh
# reads from any working directory. The weights go straight from Hugging Face to the VM.
set -euo pipefail
export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

here="$(cd "$(dirname "$0")" && pwd)"
s=pika
conf="$HOME/.config/colab-pika-h3-flex"
mkdir -p "$conf" && chmod 700 "$conf"
[ -s "$conf/api_key" ] || (umask 077 && openssl rand -hex 32 >"$conf/api_key")

# colab exec occasionally stops answering; a local hard timeout keeps the driver moving.
with_timeout() {
  local limit=$1
  shift
  if command -v gtimeout >/dev/null; then gtimeout -k 5 "$limit" "$@"; else "$@"; fi
}

run() { with_timeout "$((${2:-120} + 60))" colab exec -s "$s" --timeout "${2:-120}" -f "$1"; }
# colab exec also drops its connection now and then ("Connection was lost"); retry the steps that are
# safe to repeat (not install.py, which would start a second download).
retry() { for _ in 1 2 3; do run "$@" && return 0; sleep 10; done; return 1; }
# Pika only qualified the H100 80GB; an A100 80GB has the same memory. Colab answers 503 while it has no
# A100 free, which usually passes within minutes, so keep asking for up to 20 minutes.
for _ in $(seq 1 20); do
  colab sessions 2>&1 | grep -q "\[$s\]" && break
  colab new -s "$s" --gpu "${PIKA_GPU:-H100}" --high-mem >/dev/null 2>&1 ||
    colab new -s "$s" --gpu A100 --high-mem >/dev/null 2>&1 || sleep 60
done
colab sessions 2>&1 | grep -q "\[$s\]" || { echo "no GPU runtime available; try again later" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
# Ship the gateway and the key without putting the key on any command line.
python3 - "$here/gateway.py" "$conf/api_key" >"$work/push.py" <<'PY'
import sys
gateway, key = open(sys.argv[1]).read(), open(sys.argv[2]).read().strip()
print(f"""import os
os.makedirs('/content/pika', exist_ok=True)
open('/content/pika/gateway.py', 'w').write({gateway!r})
fd = os.open('/content/pika/api_key', os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
os.write(fd, {key!r}.encode()); os.close(fd)
print('pushed')""")
PY
retry "$work/push.py" 60 >/dev/null

cat >"$work/ready.py" <<'PY'
import os
done = all(os.path.exists(f"/content/pika/{m}") for m in ("installed", "downloaded"))
print("READY" if done else "RUNNING" if os.path.exists("/content/pika/download.log") else "NONE")
PY
# Start install/download once; later runs only wait for the markers.
if run "$work/ready.py" 60 2>/dev/null | grep -q NONE; then run "$here/install.py" 60 >/dev/null; fi
# Wait for the install/download markers, but fail instead of polling forever (60 min budget).
for _ in $(seq 1 120); do
  run "$work/ready.py" 60 2>/dev/null | grep -q READY && break
  sleep 30
done
run "$work/ready.py" 120 2>/dev/null | grep -q READY || {
  echo "install/download did not finish; see /content/pika/{install,download}.log" >&2
  exit 1
}

url="$(retry "$here/serve.py" 150 2>/dev/null | grep -oE 'https://[a-z0-9-]+\.trycloudflare\.com' | tail -1)"
[ -n "$url" ] || { echo "no tunnel url" >&2; exit 1; }
# A new quick-tunnel name takes a while to resolve here; wait so the first request does not fail.
for _ in $(seq 1 36); do
  curl -fs --max-time 10 "$url/health" >/dev/null && break
  sleep 5
done
# Keep the repository .env current so generate.sh picks up the new tunnel URL from any directory.
repo_env="$(cd "$here/../.." && pwd)/.env"
touch "$repo_env"
sed -i '' -e '/^PIKA_API_BASE=/d' -e '/^PIKA_API_KEY=/d' "$repo_env"
{ echo "PIKA_API_BASE=$url"; echo "PIKA_API_KEY=$(cat "$conf/api_key")"; } >>"$repo_env"
echo "endpoint: $url"
echo "use: tools/colab-pika-h3-flex/generate.sh 'a cat yawning' cat.mp4"
