#!/usr/bin/env python3
"""Job API in front of the `pika-h3-pro` CLI (runs on the Colab VM).

Every request except /health needs `Authorization: Bearer <key>` (key read from /content/pika/api_key).

    POST /v1/videos                 multipart: prompt, task?, resolution, aspect, duration, seed, cpu_offload,
                                    files first_image, last_image, ref (repeatable, in order) -> 202 {"id"}
    GET  /v1/videos                 {"active": jobs queued or running}
    GET  /v1/videos/<id>            {"id","status": queued|running|succeeded|failed, "error"?}
    GET  /v1/videos/<id>/content    the MP4 (H.264 + AAC, 24 fps)
    GET  /v1/videos/<id>/metadata   the CLI's JSON metadata
    GET  /health                    no auth, for readiness checks

Generation takes minutes and the quick tunnel cuts silent responses after 100 s, so clients poll.
Jobs run one at a time (one request per GPU). Standard library only.
"""

import hmac
import json
import mimetypes
import os
import queue
import re
import shutil
import subprocess
import sys
import threading
import time
import uuid
from email.parser import BytesParser
from email.policy import HTTP
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

# PIKA_* overrides exist so the routing can be tested without a GPU.
ROOT = Path(os.environ.get("PIKA_ROOT", "/content/pika"))
KEY = (ROOT / "api_key").read_text().strip()
CLI = os.environ.get("PIKA_CLI", str(ROOT / "venv/bin/pika-h3-pro"))
MODEL_REVISION = "0ddf55ad7067ca1e76812253bd7b4600df87f759"
OPTIONS = {"resolution": "480p", "aspect": "16:9", "duration": "5", "seed": "42"}
MAX_UPLOAD = 100 * 1024**2  # Cloudflare quick tunnels reject larger request bodies anyway

jobs: dict[str, dict] = {}
pending: "queue.Queue[str]" = queue.Queue()


def ref_kind(filename: str, content_type: str) -> str:
    """image|video|audio from the part's type, else from its file extension."""
    major = content_type.split("/")[0]
    if major not in ("image", "video", "audio"):
        major = (mimetypes.guess_type(filename)[0] or "").split("/")[0]
    if major not in ("image", "video", "audio"):
        raise ValueError(f"{filename}: references must be images, videos or audio")
    return major


def infer_task(has_first: bool, has_last: bool, has_refs: bool) -> str:
    if has_refs:
        return "ref2va"
    return {(False, False): "t2va", (True, False): "i2va", (False, True): "l2va", (True, True): "fl2va"}[
        (has_first, has_last)
    ]


def command(job: dict, output: Path, offload: bool) -> list[str]:
    fields, work = job["fields"], job["dir"]
    args = [CLI, "--task", job["task"], "--prompt-file", str(work / "prompt.txt"),
            "--model-repo", "Pika-Labs/Pika-H3-Flex", "--revision", MODEL_REVISION, "--output", str(output)]
    for name in ("resolution", "aspect", "duration", "seed"):
        args += [f"--{name}", fields.get(name) or OPTIONS[name]]
    for name in ("first_image", "last_image"):
        if name in job["files"]:
            args += [f"--{name.replace('_', '-')}", str(job["files"][name])]
    for kind, path in job["refs"]:
        args += ["--ref", f"{kind}:{path}"]
    return args + (["--cpu-offload"] if offload else [])


def run(job: dict) -> None:
    work = job["dir"]
    offload = job["fields"].get("cpu_offload") in ("1", "true", "yes")
    for attempt in ("first", "offload"):
        output = work / f"{attempt}.mp4"
        log = work / f"{attempt}.log"
        with log.open("w") as handle:
            code = subprocess.run(command(job, output, offload), stdout=handle, stderr=subprocess.STDOUT,
                                  cwd=work).returncode
        if code == 0:
            job.update(status="succeeded", video=output, metadata=output.with_suffix(".json"))
            return
        text = log.read_text(errors="replace")
        # 480p/5 s fits an 80 GB GPU; longer or larger requests may not, so retry once with offload.
        if offload or "OutOfMemoryError" not in text:
            job.update(status="failed", error=text[-2000:])
            return
        offload = True


def worker() -> None:
    while True:
        job = jobs[pending.get()]
        job["status"] = "running"
        started = time.time()
        try:
            run(job)
        except Exception as error:  # a crash must not stop the queue
            job.update(status="failed", error=f"{type(error).__name__}: {error}")
        print(f"{job['id']} {job['task']} {job['status']} {time.time() - started:.0f}s", file=sys.stderr, flush=True)


def parse_form(content_type: str, raw: bytes) -> tuple[dict, list[tuple[str, str, str, bytes]]]:
    """(fields, files as (field name, filename, content type, data)) from multipart/form-data."""
    message = BytesParser(policy=HTTP).parsebytes(b"Content-Type: " + content_type.encode() + b"\r\n\r\n" + raw)
    fields, files = {}, []
    for part in message.iter_parts():
        name = part.get_param("name", header="content-disposition") or ""
        body = part.get_payload(decode=True) or b""
        if part.get_filename():
            files.append((name, part.get_filename(), part.get_content_type(), body))
        else:
            fields[name] = body.decode(errors="replace")
    return fields, files


def create_job(fields: dict, files: list[tuple[str, str, str, bytes]]) -> dict:
    prompt = fields.get("prompt", "")
    if not prompt.strip():
        raise ValueError("prompt is required")
    job_id = uuid.uuid4().hex[:12]
    work = ROOT / "jobs" / job_id
    work.mkdir(parents=True)
    (work / "prompt.txt").write_text(prompt, encoding="utf-8")
    named, refs = {}, []
    for index, (name, filename, content_type, data) in enumerate(files):
        # Keep only the extension: the CLI reads media by suffix and the client's name is untrusted.
        path = work / f"{index}-{name}{Path(filename).suffix.lower()[:8]}"
        path.write_bytes(data)
        if name in ("first_image", "last_image"):
            named[name] = path
        elif name == "ref":
            refs.append((ref_kind(filename, content_type), path))
        else:
            raise ValueError(f"unknown file field {name!r}")
    task = fields.get("task") or infer_task("first_image" in named, "last_image" in named, bool(refs))
    return {"id": job_id, "status": "queued", "task": task, "fields": fields, "files": named, "refs": refs,
            "dir": work, "created": int(time.time())}


class Handler(BaseHTTPRequestHandler):
    def send_json(self, status: int, payload: dict) -> None:
        data = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def send_file(self, path: Path, content_type: str) -> None:
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(path.stat().st_size))
        self.end_headers()
        with path.open("rb") as handle:
            shutil.copyfileobj(handle, self.wfile)

    def authorized(self) -> bool:
        header = self.headers.get("Authorization", "")
        token = header[7:] if header.startswith("Bearer ") else ""
        if hmac.compare_digest(token.encode(), KEY.encode()):
            return True
        self.send_json(401, {"error": "authorization failed"})
        return False

    def do_GET(self) -> None:
        if self.path == "/health":
            self.send_json(200, {"status": "ok"})
            return
        if not self.authorized():
            return
        if self.path == "/v1/videos":  # lets a client stop the VM once nothing is left to run
            self.send_json(200, {"active": sum(job["status"] in ("queued", "running") for job in jobs.values())})
            return
        match = re.fullmatch(r"/v1/videos/([0-9a-f]{12})(/content|/metadata)?", self.path)
        job = jobs.get(match.group(1)) if match else None
        if job is None:
            self.send_json(404, {"error": "not found"})
        elif match.group(2) is None:
            self.send_json(200, {key: job[key] for key in ("id", "status", "task", "created", "error") if key in job})
        elif job["status"] != "succeeded":
            self.send_json(409, {"error": f"job is {job['status']}"})
        elif match.group(2) == "/content":
            self.send_file(job["video"], "video/mp4")
        else:
            self.send_file(job["metadata"], "application/json")

    def do_POST(self) -> None:
        if not self.authorized():
            return
        if self.path != "/v1/videos":
            self.send_json(404, {"error": "not found"})
            return
        length = int(self.headers.get("Content-Length", 0))
        content_type = self.headers.get("Content-Type", "")
        if length > MAX_UPLOAD or not content_type.startswith("multipart/form-data"):
            self.send_json(400, {"error": "send multipart/form-data under 100 MB"})
            return
        try:
            job = create_job(*parse_form(content_type, self.rfile.read(length)))
        except ValueError as error:
            self.send_json(400, {"error": str(error)})
            return
        jobs[job["id"]] = job
        pending.put(job["id"])
        self.send_json(202, {"id": job["id"], "status": job["status"], "task": job["task"]})

    def log_message(self, *args) -> None:  # requests are not logged with headers (no key leakage)
        pass


if __name__ == "__main__":
    threading.Thread(target=worker, daemon=True).start()
    # 8080 is taken by Colab's own web proxy on the VM.
    ThreadingHTTPServer(("127.0.0.1", int(os.environ.get("PIKA_PORT", "18081"))), Handler).serve_forever()
