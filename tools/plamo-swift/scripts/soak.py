"""Soak `serve`: the cold/warm/lukewarm transitions and a footprint that must not grow.

Run from tools/plamo-swift after `swift build -c release`; uses port 18559 so the live server on
18555 keeps serving. Every prompt carries a nonce, because a repeated prompt is answered from the
completion cache in about a millisecond and would hide the reload.

    python3 scripts/soak.py [cycles] [continuous]

The default is 30 cycles and 240 continuous requests, about eight minutes. The tables it prints are
what the README quotes, so a number there is a measurement rather than a guess.
"""
import json
import os
import re
import statistics
import subprocess
import sys
import time
import urllib.request

BIN = ".build/release/plamo-swift"
MODEL = os.path.expanduser("~/.local/share/plamo-translate/plamo-3-translate-0.8b-mlx-4bit")
BLOB = os.path.join(os.environ.get("TMPDIR", "/tmp"), "plamo-tokenizer.bin")
PORT = 18559
LUKEWARM = 4
CYCLES = int(sys.argv[1]) if len(sys.argv) > 1 else 30
CONTINUOUS = int(sys.argv[2]) if len(sys.argv) > 2 else 240
# The idle check runs every 5 s, so the release lands within that after `LUKEWARM`.
SETTLE = LUKEWARM + 7
PROMPT = (
    "<|plamo:tag|>input<|plamo:key|>lang<|plamo:val|>ja<|plamo:msg|>こんにちは、世界。{nonce}"
    "<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>en<|plamo:msg|>"
)


def post(nonce, max_tokens=24):
    body = {
        "prompt": PROMPT.format(nonce=f" {nonce}"),
        "max_tokens": max_tokens,
        "temperature": 0,
        "cache": False,
    }
    request = urllib.request.Request(
        f"http://127.0.0.1:{PORT}/v1/completions",
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    start = time.time()
    with urllib.request.urlopen(request, timeout=180) as response:
        json.load(response)
    return time.time() - start


def footprint(pid):
    rss = int(subprocess.run(["ps", "-o", "rss=", "-p", str(pid)],
                             capture_output=True, text=True).stdout.strip()) / 1024
    text = subprocess.run(["footprint", "-p", str(pid)], capture_output=True, text=True).stdout
    match = re.search(r"phys_footprint: ([0-9.]+) (KB|MB|GB)", text)
    scale = {"KB": 1 / 1024, "MB": 1, "GB": 1024}[match.group(2)]
    return rss, float(match.group(1)) * scale


def wait_ready(deadline=180):
    start = time.time()
    while time.time() - start < deadline:
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{PORT}/v1/models", timeout=1)
            return time.time() - start
        except Exception:
            time.sleep(0.02)
    raise SystemExit("server never became ready")


def marks(log):
    """`name: milliseconds since process start` from the PLAMO_TIMING lines."""
    found = {}
    for line in log.splitlines():
        match = re.match(r"plamo-swift timing: t\+\s*([0-9.]+) (\S+)", line)
        if match:
            found.setdefault(match.group(2), float(match.group(1)))
    return found


def table(header, rows):
    print("| " + " | ".join(header) + " |")
    print("| " + " | ".join("---" for _ in header) + " |")
    for row in rows:
        print("| " + " | ".join(row) + " |")
    print(flush=True)


stamp = int(time.time())
process = subprocess.Popen(
    [BIN, "serve", "--model", MODEL, "--tokenizer", BLOB, "--port", str(PORT),
     "--idle-seconds", "0", "--lukewarm-seconds", str(LUKEWARM)],
    stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
    env={**os.environ, "PLAMO_TIMING": "1"})

ready = wait_ready()
cold_first = post(f"cold-{stamp}")
warm_latency = [cold_first] + [post(f"warm-{stamp}-{i}") for i in range(3)]
warm_rss, warm_phys = footprint(process.pid)
time.sleep(SETTLE)
lukewarm_rss, lukewarm_phys = footprint(process.pid)
reload_first = post(f"reload-{stamp}")
reload_next = [post(f"reload-{stamp}-{i}") for i in range(3)]

print(f"\n## transitions\n", flush=True)
table(["transition", "measured"], [
    ["cold → ready (listener up, warm-up still running)", f"{ready:.3f} s"],
    ["cold → warm (first completion in a fresh process)", f"{cold_first:.3f} s"],
    ["warm → lukewarm (release after the idle window)", f"{LUKEWARM} s idle + ≤5 s check"],
    ["lukewarm → warm (first completion after the release)", f"{reload_first:.3f} s"],
    ["lukewarm → warm (steady, median of 3)", f"{statistics.median(reload_next):.3f} s"],
    ["warm footprint", f"{warm_rss:.0f} MB rss / {warm_phys:.0f} MB phys"],
    ["lukewarm footprint", f"{lukewarm_rss:.0f} MB rss / {lukewarm_phys:.0f} MB phys"],
])

cycles = []
for cycle in range(CYCLES):
    for i in range(3):
        post(f"cycle-{cycle}-{i}")
    warm = footprint(process.pid)
    time.sleep(SETTLE)
    cold = footprint(process.pid)
    cycles.append((warm[1], cold[1]))
    if cycle % 5 == 0 or cycle == CYCLES - 1:
        print(f"cycle {cycle + 1:3d}/{CYCLES}: warm {warm[1]:.0f} MB  "
              f"lukewarm {cold[1]:.0f} MB phys", flush=True)

stream = []
for index in range(CONTINUOUS):
    post(f"stream-{stamp}-{index}")
    if index % 40 == 0 or index == CONTINUOUS - 1:
        stream.append((index + 1, *footprint(process.pid)))
        print(f"stream {index + 1:4d}/{CONTINUOUS}: {stream[-1][1]:.0f} MB rss  "
              f"{stream[-1][2]:.0f} MB phys", flush=True)

print(f"\n## soak\n", flush=True)
table(["phase", "first", "last", "min", "max", "drift"], [
    ["warm, after a burst", f"{cycles[0][0]:.0f}", f"{cycles[-1][0]:.0f}",
     f"{min(c[0] for c in cycles):.0f}", f"{max(c[0] for c in cycles):.0f}",
     f"{cycles[-1][0] - cycles[0][0]:+.0f} MB"],
    ["lukewarm, after the release", f"{cycles[0][1]:.0f}", f"{cycles[-1][1]:.0f}",
     f"{min(c[1] for c in cycles):.0f}", f"{max(c[1] for c in cycles):.0f}",
     f"{cycles[-1][1] - cycles[0][1]:+.0f} MB"],
    ["continuous, physical", f"{stream[0][2]:.0f}", f"{stream[-1][2]:.0f}",
     f"{min(s[2] for s in stream):.0f}", f"{max(s[2] for s in stream):.0f}",
     f"{stream[-1][2] - stream[0][2]:+.0f} MB"],
    ["continuous, resident", f"{stream[0][1]:.0f}", f"{stream[-1][1]:.0f}",
     f"{min(s[1] for s in stream):.0f}", f"{max(s[1] for s in stream):.0f}",
     f"{stream[-1][1] - stream[0][1]:+.0f} MB"],
])

process.terminate()
try:
    log, _ = process.communicate(timeout=20)
except subprocess.TimeoutExpired:
    process.kill()
    log, _ = process.communicate()
found = marks(log)
print(f"\n## phases (process-relative, from PLAMO_TIMING)\n", flush=True)
table(["phase", "at"], [[name, f"{value:.2f} ms"] for name, value in found.items()])
if "lukewarm.begin" in found and "lukewarm" in found:
    print(f"\nrelease cost: {found['lukewarm'] - found['lukewarm.begin']:.2f} ms", flush=True)
