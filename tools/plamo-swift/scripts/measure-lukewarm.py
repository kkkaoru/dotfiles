"""Measure the lukewarm state: footprint warm vs lukewarm, and the first request after it.

Run from tools/plamo-swift after `swift build -c release`; uses port 18558 so the live server on
18555 keeps serving. Every prompt carries a nonce, because a repeated prompt is answered from the
completion cache in about a millisecond and would hide the reload.
"""
import json
import os
import statistics
import subprocess
import time
import urllib.request

BIN = ".build/release/plamo-swift"
MODEL = os.path.expanduser("~/.local/share/plamo-translate/plamo-3-translate-0.8b-mlx-4bit")
BLOB = os.path.join(os.environ.get("TMPDIR", "/tmp"), "plamo-tokenizer.bin")
PORT = 18558
LUKEWARM = 8
PROMPT = (
    "<|plamo:tag|>input<|plamo:key|>lang<|plamo:val|>ja<|plamo:msg|>こんにちは、世界。{nonce}"
    "<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>en<|plamo:msg|>"
)


def post(nonce, max_tokens=24):
    body = {"prompt": PROMPT.format(nonce=f" {nonce}"), "max_tokens": max_tokens,
            "temperature": 0, "cache": False}
    request = urllib.request.Request(
        f"http://127.0.0.1:{PORT}/v1/completions",
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    start = time.time()
    with urllib.request.urlopen(request, timeout=180) as response:
        json.load(response)
    return time.time() - start


def memory(pid):
    rss = int(subprocess.run(["ps", "-o", "rss=", "-p", str(pid)],
                             capture_output=True, text=True).stdout.strip()) / 1024
    text = subprocess.run(["footprint", "-p", str(pid)], capture_output=True, text=True).stdout
    phys = next((line.split("phys_footprint:")[1].strip()
                 for line in text.splitlines() if "phys_footprint:" in line), "?")
    return rss, phys


def wait_ready(deadline=180):
    start = time.time()
    while time.time() - start < deadline:
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{PORT}/v1/models", timeout=1)
            return time.time() - start
        except Exception:
            time.sleep(0.05)
    raise SystemExit("server never became ready")


stamp = int(time.time())
process = subprocess.Popen(
    [BIN, "serve", "--model", MODEL, "--tokenizer", BLOB, "--port", str(PORT),
     "--idle-seconds", "600", "--lukewarm-seconds", str(LUKEWARM)],
    stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
print(f"cold: ready {wait_ready():.3f}s", flush=True)
cold_first = post(f"cold-{stamp}")
print(f"cold: first completion {cold_first:.3f}s", flush=True)

warm = [post(f"warm-{stamp}-{i}") for i in range(4)]
print("warm latencies " + " ".join(f"{value:.3f}" for value in warm), flush=True)
warm_rss, warm_phys = memory(process.pid)
print(f"warm      rss {warm_rss:.0f} MB  phys {warm_phys}", flush=True)

time.sleep(LUKEWARM + 4)
lukewarm_rss, lukewarm_phys = memory(process.pid)
print(f"lukewarm  rss {lukewarm_rss:.0f} MB  phys {lukewarm_phys}", flush=True)

reload_latency = post(f"reload-{stamp}")
print(f"first after lukewarm {reload_latency:.3f}s", flush=True)
after = [post(f"after-{stamp}-{i}") for i in range(4)]
print("after-reload latencies " + " ".join(f"{value:.3f}" for value in after), flush=True)
print(f"steady warm median {statistics.median(warm[1:]):.3f}s  "
      f"after reload median {statistics.median(after[1:]):.3f}s", flush=True)
print(f"lukewarm reload vs cold start: {reload_latency:.3f}s vs {cold_first:.3f}s", flush=True)

process.terminate()
try:
    log, _ = process.communicate(timeout=15)
except subprocess.TimeoutExpired:
    process.kill()
    log, _ = process.communicate()
print("--- server log")
print("\n".join(line for line in log.splitlines() if "ready" in line or "lukewarm" in line))
