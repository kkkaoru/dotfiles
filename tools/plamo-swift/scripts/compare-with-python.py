"""Time the same steps on `plamo-swift serve` and on the Python `plamo-mlx-server`.

Run from tools/plamo-swift; uses ports 18601 and 18600 so the live server on 18555 keeps serving.
Each column is a fresh process: spawn to `/v1/models` answering, the first completion, five steady
completions, then the completion after a 12 s idle gap (a lukewarm reload on the Swift side, a plain
request on the Python side, which releases nothing), with the physical footprint at each state.

Every prompt carries a nonce, because a repeated prompt is answered from the completion cache in
about a millisecond and would hide the work. The tables in
docs/plamo-translate-swift-vs-python.md are this script's output.
"""
import json
import os
import signal
import statistics
import subprocess
import time
import urllib.request

HOME = os.path.expanduser("~")
MODEL = f"{HOME}/.local/share/plamo-translate/plamo-3-translate-0.8b-mlx-4bit"
SWIFT = f"{HOME}/.local/share/plamo-swift/plamo-swift"
PYBIN = f"{HOME}/.local/share/uv/tools/mlx-lm/bin/python"
PYSERVER = f"{HOME}/.local/share/plamo-translate/plamo-mlx-server.py"
STOP = ["<|plamo:tag|>", "<|plamo:bos|>"]
PROMPT = (
    "<|plamo:tag|>dataset<|plamo:msg|>translation<|plamo:tag|>input<|plamo:msg|>おはようございます"
    "<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>ja<|plamo:key|>form<|plamo:val|>polite"
    "<|plamo:msg|>"
)
IDLE_SECONDS = 12


def wait_ready(port: int, deadline: float) -> bool:
    while time.perf_counter() < deadline:
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/v1/models", timeout=0.5) as reply:
                if reply.status == 200:
                    return True
        except Exception:
            pass
        time.sleep(0.02)
    return False


def complete(port: int, nonce: str) -> tuple[float, str]:
    body = json.dumps(
        {
            "prompt": PROMPT.replace("おはようございます", f"おはようございます{nonce}"),
            "max_tokens": 32,
            "temperature": 0,
            "stop": STOP,
            "cache": False,
        }
    ).encode()
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}/v1/completions",
        data=body,
        headers={"content-type": "application/json"},
    )
    start = time.perf_counter()
    with urllib.request.urlopen(request, timeout=180) as reply:
        payload = json.loads(reply.read())
    return time.perf_counter() - start, payload["choices"][0]["text"]


def footprint(pid: int) -> str:
    output = subprocess.run(["footprint", "-p", str(pid)], capture_output=True, text=True).stdout
    for line in output.splitlines():
        if "phys_footprint:" in line and "peak" not in line:
            return line.split(":", 1)[1].strip()
    return "?"


def run(name: str, command: list[str], port: int) -> dict:
    started = time.perf_counter()
    process = subprocess.Popen(
        command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True
    )
    result: dict = {"name": name, "pid": process.pid}
    if not wait_ready(port, started + 180):
        result["ready_s"] = None
        process.kill()
        return result
    result["ready_s"] = round(time.perf_counter() - started, 3)

    first, text = complete(port, "")
    result["first_s"] = round(first, 3)
    result["first_text"] = text.strip()[:60]

    steady = [complete(port, f" {index}")[0] for index in range(5)]
    result["steady_s"] = [round(value, 3) for value in steady]
    result["steady_median_s"] = round(statistics.median(steady), 3)
    result["fp_warm"] = footprint(process.pid)

    time.sleep(IDLE_SECONDS)
    result["fp_idle"] = footprint(process.pid)
    after, _ = complete(port, " idle")
    result["after_idle_s"] = round(after, 3)
    result["fp_after"] = footprint(process.pid)

    try:
        os.killpg(os.getpgid(process.pid), signal.SIGTERM)
    except ProcessLookupError:
        pass
    process.wait(timeout=30)
    return result


def main() -> None:
    results = [
        run(
            "swift",
            [SWIFT, "serve", "--model", MODEL, "--port", "18601", "--lukewarm-seconds", "2"],
            18601,
        ),
        run(
            "python",
            [PYBIN, PYSERVER, "--model", MODEL, "--port", "18600", "--trust-remote-code"],
            18600,
        ),
    ]
    print(json.dumps(results, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
