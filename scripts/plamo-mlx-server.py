#!/usr/bin/env python3
"""mlx_lm.server tuned for tools/*-plamo-translate: a listen backlog that survives their burst of
requests, and a persistent cache of greedy completions.

Why the backlog: eight translations can arrive at once, and CPython's ThreadingHTTPServer keeps
`request_queue_size = 5`, so the kernel drops the extra connections and the client sees
`ECONNRESET`; a dropped request leaves that line untranslated. PFN's serve.py defaults (shorter
prefill steps, one prompt cache) measured no faster for these short prompts, so the rest is
mlx_lm.server's own behaviour.

Why the cache: both mods re-translate the same text constantly — a redrawn block, a scrolled
screen, a resumed session, the same paragraph in a later turn. Greedy decoding (temperature 0) is
deterministic, so the same prompt always yields the same translation: serving it from disk is
byte-identical to running the model, and costs about a millisecond instead of a hundred or more.
Requests that ask for sampling, or that pass `"cache": false`, bypass it.

Run: plamo-mlx-server --model <mlx-dir> --port 18555 --trust-remote-code
"""

import hashlib
import http.server
import io
import json
import os
import sys
import threading
import time

http.server.ThreadingHTTPServer.request_queue_size = 512

from mlx_lm import server  # noqa: E402 - after the class attribute is set

CACHE_DIR = os.path.join(os.path.expanduser("~"), ".cache", "plamo-translate")
CACHE_LIMIT = 20000
COMPLETIONS = "/v1/completions"
# The model is ~1 GB resident and nothing needs it between translations, so an idle server exits
# and the Mac gets the memory back. 0 disables the exit.
IDLE_MINUTES = float(os.environ.get("PLAMO_IDLE_MINUTES", "10") or 0)
IDLE_CHECK_SECONDS = 15


class Activity:
    """When the last request finished and how many are running, so an idle server can exit."""

    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.in_flight = 0
        self.last = time.monotonic()

    def enter(self) -> None:
        with self.lock:
            self.in_flight += 1
            self.last = time.monotonic()

    def leave(self) -> None:
        with self.lock:
            self.in_flight -= 1
            self.last = time.monotonic()

    def idle(self, minutes: float) -> bool:
        with self.lock:
            return self.in_flight == 0 and time.monotonic() - self.last >= minutes * 60


def watch_idle(activity: Activity, minutes: float) -> None:
    while True:
        time.sleep(min(IDLE_CHECK_SECONDS, minutes * 60))
        if activity.idle(minutes):
            # Every cached completion is already on disk, so there is nothing to flush. The next
            # translation starts the server again, paying the 2.2 s load and 3.9 s first inference.
            os._exit(0)


def cache_file() -> str:
    """One cache file per model directory version, so new weights never serve old translations."""
    model = None
    argv = sys.argv
    for index, argument in enumerate(argv):
        if argument == "--model" and index + 1 < len(argv):
            model = argv[index + 1]
        elif argument.startswith("--model="):
            model = argument.split("=", 1)[1]
    tag = "default"
    if model is not None:
        config = os.path.join(model, "config.json")
        try:
            stat = os.stat(config)
            digest = hashlib.sha256(f"{model}\x00{stat.st_mtime_ns}\x00{stat.st_size}".encode())
            tag = digest.hexdigest()[:12]
        except OSError:
            tag = hashlib.sha256(model.encode()).hexdigest()[:12]
    return os.path.join(CACHE_DIR, f"completions-{tag}.jsonl")


class CompletionCache:
    """Greedy completions on disk, keyed by the prompt and its token limit."""

    def __init__(self, path: str, limit: int) -> None:
        self.path = path
        self.limit = limit
        self.entries: dict[str, str] = {}
        self.pending: dict[str, threading.Event] = {}
        self.lock = threading.Lock()
        self.hits = 0
        self.misses = 0
        self._load()

    def _load(self) -> None:
        if not os.path.exists(self.path):
            return
        try:
            with open(self.path, encoding="utf-8") as handle:
                lines = handle.readlines()[-self.limit :]
        except OSError:
            return
        for line in lines:
            try:
                entry = json.loads(line)
            except ValueError:
                continue
            key, value = entry.get("k"), entry.get("v")
            if isinstance(key, str) and isinstance(value, str):
                self.entries[key] = value

    @staticmethod
    def key(prompt: str, max_tokens: int, stop=None) -> str:
        # `stop` changes the text the model returns for the same prompt, so it is part of the key.
        digest = hashlib.sha256(
            f"{max_tokens}\x00{json.dumps(stop, ensure_ascii=False, sort_keys=True)}\x00{prompt}".encode()
        )
        return digest.hexdigest()[:32]

    def get(self, key: str) -> str | None:
        with self.lock:
            value = self.entries.get(key)
            self.hits += 1 if value is not None else 0
            self.misses += 0 if value is not None else 1
            return value

    def claim(self, key: str) -> threading.Event | None:
        """Claim a generation. Returns None when this request should run it, else the event to wait
        on: a redraw can ask for the same line twice at once, and one generation answers both."""
        with self.lock:
            event = self.pending.get(key)
            if event is not None:
                return event
            self.pending[key] = threading.Event()
            return None

    def release(self, key: str) -> None:
        with self.lock:
            event = self.pending.pop(key, None)
        if event is not None:
            event.set()

    def put(self, key: str, value: str) -> None:
        with self.lock:
            if key in self.entries:
                return
            self.entries[key] = value
            if len(self.entries) > self.limit:
                self._compact()
            try:
                os.makedirs(CACHE_DIR, exist_ok=True)
                with open(self.path, "a", encoding="utf-8") as handle:
                    handle.write(json.dumps({"k": key, "v": value}, ensure_ascii=False) + "\n")
            except OSError:
                pass

    def _compact(self) -> None:
        """Keep the newest half, so the file cannot grow without bound."""
        keep = list(self.entries.items())[-self.limit // 2 :]
        self.entries = dict(keep)
        try:
            os.makedirs(CACHE_DIR, exist_ok=True)
            with open(self.path, "w", encoding="utf-8") as handle:
                for key, value in keep:
                    handle.write(json.dumps({"k": key, "v": value}, ensure_ascii=False) + "\n")
        except OSError:
            pass


class Capture:
    """Passes writes through while keeping a copy, to learn what the handler answered."""

    def __init__(self, stream) -> None:
        self.stream = stream
        self.chunks: list[bytes] = []

    def write(self, data: bytes) -> int:
        self.chunks.append(bytes(data))
        return self.stream.write(data)

    def __getattr__(self, name):
        return getattr(self.stream, name)


def install(cache: CompletionCache, activity: Activity) -> None:
    original_post = server.APIHandler.do_POST
    # Resolved before the assignment below, or the fallback would call this wrapper again.
    original_get = server.APIHandler.do_GET

    def do_post(self) -> None:
        activity.enter()
        try:
            handle_post(self)
        finally:
            activity.leave()

    def handle_post(self) -> None:
        if self.path != COMPLETIONS:
            original_post(self)
            return
        length = int(self.headers.get("Content-Length", 0) or 0)
        raw = self.rfile.read(length)
        self.rfile = io.BytesIO(raw)  # the original handler still has to read the body
        try:
            request = json.loads(raw or b"{}")
        except ValueError:
            original_post(self)
            return

        prompt = request.get("prompt")
        greedy = float(request.get("temperature", 0) or 0) == 0
        cached = greedy and request.get("cache", True) and isinstance(prompt, str)
        key = (
            cache.key(prompt, int(request.get("max_tokens", 512)), request.get("stop"))
            if cached
            else None
        )
        owner = False
        if key is not None:
            hit = cache.get(key)
            if hit is not None:
                self.send_hit(hit)
                return
            waiter = cache.claim(key)
            if waiter is None:
                owner = True
            else:
                # Another request is decoding this exact text; a redraw asks for the same line
                # twice, and one generation answers both.
                waiter.wait(timeout=120)
                hit = cache.get(key)
                if hit is not None:
                    self.send_hit(hit)
                    return
                owner = True  # that generation failed, so this request runs its own

        capture = Capture(self.wfile)
        self.wfile = capture
        try:
            original_post(self)
        finally:
            self.wfile = capture.stream
        if not owner:
            return
        try:
            # end_headers() flushes the status line and headers through the same stream, so the
            # body starts after the blank line.
            _, _, body = b"".join(capture.chunks).partition(b"\r\n\r\n")
            try:
                text = json.loads(body)["choices"][0]["text"]
            except (ValueError, KeyError, IndexError, TypeError):
                return
            if isinstance(text, str):
                cache.put(key, text)
        finally:
            cache.release(key)

    def send_hit(self, text: str) -> None:
        body = json.dumps(
            {
                "choices": [{"text": text, "index": 0, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0},
            },
            ensure_ascii=False,
        ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("X-Plamo-Cache", "hit")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:
        activity.enter()
        try:
            handle_get(self)
        finally:
            activity.leave()

    def handle_get(self) -> None:
        if self.path == "/cache/stats":
            body = json.dumps(
                {"entries": len(cache.entries), "hits": cache.hits, "misses": cache.misses}
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        original_get(self)

    server.APIHandler.do_POST = do_post
    server.APIHandler.do_GET = do_GET
    server.APIHandler.send_hit = send_hit


def main() -> int:
    path = cache_file()
    cache = CompletionCache(path, CACHE_LIMIT)
    activity = Activity()
    install(cache, activity)
    if IDLE_MINUTES > 0:
        threading.Thread(target=watch_idle, args=(activity, IDLE_MINUTES), daemon=True).start()
    print(f"completion cache: {len(cache.entries)} entries from {path}", flush=True)
    return server.main()


if __name__ == "__main__":
    sys.exit(main())
