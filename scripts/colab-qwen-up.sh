#!/usr/bin/env bash
# Bring up Qwen3.8-27B on a fresh Colab A100 via the `colab` CLI and connect pi to it.
# Reuses the persistent key in ~/.pi/agent/auth.json. Stop later with: colab stop -s qwen
set -euo pipefail
export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

root="$(cd "$(dirname "$0")/.." && pwd)"
s=qwen
work="$(mktemp -d)"
"$root/scripts/colab-qwen-ensure-key.sh"
key="$(jq -er '.["google-colab"].key' "$root/.pi/agent/auth.json")"

run() { colab exec -s "$s" --timeout "${2:-120}" -f "$1"; }

colab sessions 2>&1 | grep -q "\[$s\]" || colab new -s "$s" --gpu A100

cat >"$work/install.py" <<'PY'
import subprocess
subprocess.Popen("rm -f /content/installed; pip install -q -U vllm && wget -q https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64 -O /usr/local/bin/cloudflared && chmod +x /usr/local/bin/cloudflared && touch /content/installed",
    shell=True, stdout=open("/content/install.log", "w"), stderr=subprocess.STDOUT, start_new_session=True)
PY
cat >"$work/installed.py" <<'PY'
import os, time
for _ in range(12):
    if os.path.exists("/content/installed"):
        print("DONE"); break
    time.sleep(10)
PY
cat >"$work/serve.py" <<PY
import subprocess
open("/content/api_key", "w").write("$key")
subprocess.Popen(["vllm", "serve", "Qwen/Qwen3.8-27B", "--served-model-name", "Qwen3.8-27B",
    "--quantization", "fp8", "--max-model-len", "65536", "--gpu-memory-utilization", "0.90",
    "--max-num-seqs", "4", "--max-num-batched-tokens", "4096", "--enforce-eager",
    "--reasoning-parser", "qwen3", "--enable-auto-tool-choice", "--tool-call-parser", "qwen3_coder",
    "--api-key", "$key", "--port", "8000"],
    stdout=open("/content/vllm.log", "w"), stderr=subprocess.STDOUT, start_new_session=True)
PY
cat >"$work/wait.py" <<'PY'
import time, urllib.request
for _ in range(10):
    try:
        urllib.request.urlopen("http://127.0.0.1:8000/health", timeout=3); print("READY"); break
    except Exception:
        time.sleep(10)
PY
cat >"$work/tunnel.py" <<'PY'
import re, subprocess, time
subprocess.Popen(["cloudflared", "tunnel", "--no-autoupdate", "--url", "http://127.0.0.1:8000"],
    stdout=open("/content/cf.log", "w"), stderr=subprocess.STDOUT, start_new_session=True)
for _ in range(30):
    time.sleep(2)
    m = re.search(r"https://[a-z0-9-]+\.trycloudflare\.com", open("/content/cf.log").read())
    if m:
        print(m.group(0)); break
PY

run "$work/install.py" 60
until run "$work/installed.py" 150 2>/dev/null | grep -q DONE; do sleep 5; done
run "$work/serve.py" 60
until run "$work/wait.py" 150 2>/dev/null | grep -q READY; do sleep 5; done
url="$(run "$work/tunnel.py" 90 2>/dev/null | grep -oE 'https://[a-z0-9-]+\.trycloudflare\.com' | tail -1)"
"$root/scripts/colab-qwen-connect.sh" "$url"
