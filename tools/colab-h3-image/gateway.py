#!/usr/bin/env python3
"""MiniMax-API-compatible front for a local SGLang MiniMax-H3-Image server (runs on the Colab VM).

Only requests with `Authorization: Bearer <key>` (key read from /content/h3/api_key) are forwarded.

    POST /v1/images/generations   MiniMax H3 shape, passed through   -> {"data": [{"b64_json": ...}]}
    POST /v1/image_generation     MiniMax classic image_generation   -> {"data": {"image_base64": [...]}, "base_resp": ...}
    GET  /health                  no auth, for readiness checks

Standard library only; one request at a time is enough for a single GPU.
"""

import hmac
import threading
import json
import math
import sys
import time
import urllib.error
import urllib.request
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

UPSTREAM = "http://127.0.0.1:30000"
KEY = open("/content/h3/api_key").read().strip()
AREA = 2048 * 2048  # default canvas for aspect_ratio requests; the model's working range starts ~4 MP


def size_for(aspect_ratio: str) -> str:
    x, y = (int(v) for v in aspect_ratio.split(":"))
    return f"{round(math.sqrt(AREA * x / y) / 32) * 32}x{round(math.sqrt(AREA * y / x) / 32) * 32}"


def upstream(body: dict) -> dict:
    request = urllib.request.Request(
        f"{UPSTREAM}/v1/images/generations",
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=1800) as response:
        return json.load(response)


class Handler(BaseHTTPRequestHandler):
    def send_json(self, status: int, payload: dict) -> None:
        data = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def authorized(self) -> bool:
        header = self.headers.get("Authorization", "")
        token = header[7:] if header.startswith("Bearer ") else ""
        return hmac.compare_digest(token.encode(), KEY.encode())

    def do_GET(self) -> None:
        if self.path == "/health":
            self.send_json(200, {"status": "ok"})
        else:
            self.send_json(404, {"error": "not found"})

    def do_POST(self) -> None:
        if not self.authorized():
            self.send_json(401, {"base_resp": {"status_code": 1004, "status_msg": "authorization failed"}})
            return
        try:
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
        except json.JSONDecodeError:
            self.send_json(400, {"base_resp": {"status_code": 2013, "status_msg": "invalid json"}})
            return
        if not isinstance(body.get("prompt"), str) or not body["prompt"].strip():
            self.send_json(400, {"base_resp": {"status_code": 2013, "status_msg": "prompt is required"}})
            return
        if self.path not in ("/v1/images/generations", "/v1/image_generation"):
            self.send_json(404, {"base_resp": {"status_code": 404, "status_msg": "not found"}})
            return
        started = time.time()
        result: dict = {}
        worker = threading.Thread(target=self.generate, args=(body, result), daemon=True)
        worker.start()
        # Cloudflare cuts responses with no headers after 100 s (HTTP 524), and generation takes longer.
        # Send 200 headers now, keep the connection alive with JSON-insignificant spaces, then the body.
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        try:
            while worker.is_alive():
                worker.join(15)
                if worker.is_alive():
                    self.wfile.write(b" ")
                    self.wfile.flush()
            self.wfile.write(json.dumps(result).encode())
        except (BrokenPipeError, ConnectionResetError):
            pass
        finally:
            print(f"{self.path} {time.time() - started:.1f}s", file=sys.stderr, flush=True)

    def generate(self, body: dict, result: dict) -> None:
        try:
            if self.path == "/v1/images/generations":
                body.setdefault("size", "2048x2048")
                body.setdefault("generator_device", "cpu")
                body["response_format"] = "b64_json"
                body.pop("model", None)
                result.update(upstream(body))
            else:
                n = max(1, min(int(body.get("n", 1)), 4))
                size = body.get("size") or size_for(body.get("aspect_ratio", "1:1"))
                images = []
                for i in range(n):
                    request = {"prompt": body["prompt"], "size": size, "response_format": "b64_json", "generator_device": "cpu"}
                    if "seed" in body:
                        request["seed"] = int(body["seed"]) + i
                    images.append(upstream(request)["data"][0]["b64_json"])
                result.update({
                    "id": uuid.uuid4().hex,
                    "data": {"image_base64": images},
                    "metadata": {"success_count": len(images), "failed_count": 0, "size": size},
                    "base_resp": {"status_code": 0, "status_msg": "success"},
                })
        except urllib.error.HTTPError as error:
            # Status is already 200 (streamed); the error is reported MiniMax-style in base_resp.
            result.update({"base_resp": {"status_code": error.code, "status_msg": error.read().decode(errors="replace")[:500]}})
        except Exception as error:
            result.update({"base_resp": {"status_code": 1000, "status_msg": f"{type(error).__name__}: {error}"[:500]}})

    def log_message(self, *args) -> None:  # requests are logged above without headers (no key leakage)
        pass


if __name__ == "__main__":
    # 8080 is taken by Colab's own web proxy on the VM.
    ThreadingHTTPServer(("127.0.0.1", 18080), Handler).serve_forever()
