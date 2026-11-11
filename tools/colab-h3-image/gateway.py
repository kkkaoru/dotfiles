#!/usr/bin/env python3
"""MiniMax-API-compatible front for a local SGLang MiniMax-H3-Image server (runs on the Colab VM).

Only requests with `Authorization: Bearer <key>` (key read from /content/h3/api_key) are forwarded.

    POST /v1/images/generations   MiniMax H3 text-to-image   {"model","prompt","size"|"aspect_ratio"+"area","n","seed"}
    POST /v1/images/edits         MiniMax H3 image editing   + {"images":[{"image_url":...}]} (JSON)
                                                             or multipart `image` files (OpenAI SDK shape)
    POST /v1/image_generation     MiniMax classic image_generation (kept for older clients)
    GET  /health                  no auth, for readiness checks

Responses use the MiniMax shape: {"created","data":[{"b64_json","size"}],"output_format","request_id"}.
`response_format` is always b64_json and the output is always PNG (that is what SGLang returns).

Standard library only; one request at a time is enough for a single GPU.
"""

import base64
import hmac
import json
import math
import os
import re
import sys
import threading
import time
import urllib.error
import urllib.request
import uuid
from email.parser import BytesParser
from email.policy import HTTP
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

# H3_UPSTREAM/H3_KEY_FILE exist so the routing can be tested without a GPU.
UPSTREAM = os.environ.get("H3_UPSTREAM", "http://127.0.0.1:30000")
KEY = open(os.environ.get("H3_KEY_FILE", "/content/h3/api_key")).read().strip()
AREAS = {"1K": 1024**2, "2K": 2048**2, "4K": 4096**2}  # MiniMax `area` levels
MAX_REFS = 16


def size_for(aspect_ratio: str, area: str) -> str:
    x, y = (int(v) for v in aspect_ratio.split(":"))
    px = AREAS.get(area, AREAS["4K"])
    return f"{round(math.sqrt(px * x / y) / 32) * 32}x{round(math.sqrt(px * y / x) / 32) * 32}"


def target_size(body: dict) -> str:
    size = body.get("size")
    if isinstance(size, str) and size and size != "auto":
        return size
    return size_for(body.get("aspect_ratio") or "1:1", body.get("area") or "4K")


def fetch_image(ref: str) -> tuple[bytes, str]:
    """MiniMax `image_url`: a public http(s) URL or a base64 data URI."""
    match = re.match(r"data:image/([a-zA-Z0-9.+-]+);base64,(.*)", ref, re.S)
    if match:
        return base64.b64decode(match.group(2)), f"image/{match.group(1)}"
    if ref.startswith(("http://", "https://")):
        with urllib.request.urlopen(ref, timeout=120) as response:
            return response.read(), response.headers.get_content_type() or "image/png"
    raise ValueError("image_url must be an http(s) URL or a base64 data URI (mm_file:// is not available)")


def multipart(fields: dict, images: list[tuple[bytes, str]]) -> tuple[bytes, str]:
    boundary = uuid.uuid4().hex
    parts = b""
    for name, value in fields.items():
        parts += f'--{boundary}\r\nContent-Disposition: form-data; name="{name}"\r\n\r\n{value}\r\n'.encode()
    for i, (data, content_type) in enumerate(images, 1):
        parts += (
            f'--{boundary}\r\nContent-Disposition: form-data; name="image"; filename="image{i}.png"\r\n'
            f"Content-Type: {content_type}\r\n\r\n"
        ).encode() + data + b"\r\n"
    return parts + f"--{boundary}--\r\n".encode(), f"multipart/form-data; boundary={boundary}"


def upstream(path: str, body: dict, images: list[tuple[bytes, str]]) -> dict:
    if images:
        # SGLang takes image editing as multipart form data with one `image` part per reference.
        payload, content_type = multipart({k: str(v) for k, v in body.items()}, images)
    else:
        payload, content_type = json.dumps(body).encode(), "application/json"
    request = urllib.request.Request(f"{UPSTREAM}/v1/images/{path}", data=payload, headers={"Content-Type": content_type})
    with urllib.request.urlopen(request, timeout=3600) as response:
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

    def read_body(self) -> tuple[dict, list[tuple[bytes, str]]]:
        """Return (fields, reference images) from a JSON or multipart/form-data body."""
        raw = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        content_type = self.headers.get("Content-Type", "")
        if not content_type.startswith("multipart/form-data"):
            return json.loads(raw or b"{}"), []
        message = BytesParser(policy=HTTP).parsebytes(b"Content-Type: " + content_type.encode() + b"\r\n\r\n" + raw)
        fields, images = {}, []
        for part in message.iter_parts():
            name = (part.get_param("name", header="content-disposition") or "").rstrip("[]")
            body = part.get_payload(decode=True) or b""
            if part.get_filename():
                if name == "image":
                    images.append((body, part.get_content_type() or "image/png"))
            else:
                fields[name] = body.decode(errors="replace")
        return fields, images

    def do_POST(self) -> None:
        if not self.authorized():
            self.send_json(401, {"base_resp": {"status_code": 1004, "status_msg": "authorization failed"}})
            return
        if self.path not in ("/v1/images/generations", "/v1/images/edits", "/v1/image_generation"):
            self.send_json(404, {"base_resp": {"status_code": 404, "status_msg": "not found"}})
            return
        try:
            body, images = self.read_body()
        except (json.JSONDecodeError, UnicodeDecodeError):
            self.send_json(400, {"base_resp": {"status_code": 2013, "status_msg": "invalid json"}})
            return
        if not isinstance(body.get("prompt"), str) or not body["prompt"].strip():
            self.send_json(400, {"base_resp": {"status_code": 2013, "status_msg": "prompt is required"}})
            return
        refs = [item.get("image_url", "") for item in body.get("images") or [] if isinstance(item, dict)]
        if len(refs) + len(images) > MAX_REFS:
            self.send_json(400, {"base_resp": {"status_code": 2013, "status_msg": f"at most {MAX_REFS} reference images"}})
            return
        if self.path == "/v1/images/edits" and not refs and not images:
            self.send_json(400, {"base_resp": {"status_code": 2013, "status_msg": "images is required for edits"}})
            return
        started = time.time()
        result: dict = {}
        worker = threading.Thread(target=self.generate, args=(body, images, refs, result), daemon=True)
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
            print(f"{self.path} refs={len(refs) + len(images)} {time.time() - started:.1f}s", file=sys.stderr, flush=True)

    def generate(self, body: dict, uploads: list[tuple[bytes, str]], refs: list[str], result: dict) -> None:
        try:
            size = target_size(body)
            count = max(1, min(int(body.get("n", 1)), 4))
            seed = int(body["seed"]) if str(body.get("seed", "")).lstrip("-").isdigit() else None
            images = list(uploads) + [fetch_image(ref) for ref in refs if ref]
            base = {"prompt": body["prompt"], "size": size, "response_format": "b64_json", "generator_device": "cpu"}
            generated = []
            for i in range(count):
                request = dict(base)
                if seed is not None:
                    request["seed"] = seed + i
                generated.append(upstream("edits" if images else "generations", request, images)["data"][0]["b64_json"])
            if self.path == "/v1/image_generation":  # MiniMax classic shape, kept for older clients
                result.update({
                    "id": uuid.uuid4().hex,
                    "data": {"image_base64": generated},
                    "metadata": {"success_count": len(generated), "failed_count": 0, "size": size},
                    "base_resp": {"status_code": 0, "status_msg": "success"},
                })
                return
            result.update({
                "created": int(time.time()),
                "data": [{"b64_json": image, "size": size} for image in generated],
                "output_format": "png",
                "request_id": uuid.uuid4().hex,
            })
        except urllib.error.HTTPError as error:
            # Status is already 200 (streamed); the error is reported MiniMax-style in base_resp.
            result.update({"base_resp": {"status_code": error.code, "status_msg": error.read().decode(errors="replace")[:500]},
                           "error": {"code": str(error.code), "message": error.reason}})
        except Exception as error:
            result.update({"base_resp": {"status_code": 1000, "status_msg": f"{type(error).__name__}: {error}"[:500]},
                           "error": {"code": "1000", "message": f"{type(error).__name__}: {error}"[:300]}})

    def log_message(self, *args) -> None:  # requests are logged above without headers (no key leakage)
        pass


if __name__ == "__main__":
    # 8080 is taken by Colab's own web proxy on the VM.
    ThreadingHTTPServer(("127.0.0.1", 18080), Handler).serve_forever()
