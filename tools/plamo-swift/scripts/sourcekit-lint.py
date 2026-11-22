#!/usr/bin/env python3
"""Report the diagnostics SourceKit produces for this package.

SourceKit-LSP is the only supported way to ask SourceKit for a file's diagnostics, so this speaks
LSP over stdio: initialize, open every Swift file, collect textDocument/publishDiagnostics, then shut
the server down. Any error or warning fails the run.

  sourcekit-lint.py [--timeout SECONDS] [--settle SECONDS] [files...]
"""
from __future__ import annotations

import argparse
import json
import queue
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import Any

SEVERITIES = {1: "error", 2: "warning", 3: "information", 4: "hint"}
FAILING = {"error", "warning"}
SETTLE_SECONDS = 10.0
TIMEOUT_SECONDS = 180.0


class Client:
    """A minimal LSP client: JSON-RPC over the server's stdin/stdout."""

    def __init__(self, process: subprocess.Popen[bytes]) -> None:
        self.process = process
        self.next_id = 1
        self.incoming: queue.Queue[dict[str, Any]] = queue.Queue()
        self.closed = threading.Event()
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self) -> None:
        stream = self.process.stdout
        if stream is None:
            return
        try:
            while True:
                length = 0
                while True:
                    line = stream.readline()
                    if not line:
                        return
                    text = line.decode("utf-8", "replace").strip()
                    if not text:
                        break
                    key, _, value = text.partition(":")
                    if key.lower() == "content-length":
                        length = int(value.strip())
                if length <= 0:
                    continue
                body = stream.read(length)
                if not body:
                    return
                self.incoming.put(json.loads(body))
        finally:
            self.closed.set()

    def send(self, payload: dict[str, Any]) -> None:
        stdin = self.process.stdin
        if stdin is None:
            raise RuntimeError("sourcekit-lsp has no stdin")
        data = json.dumps(payload).encode()
        stdin.write(b"Content-Length: %d\r\n\r\n" % len(data) + data)
        stdin.flush()

    def notify(self, method: str, params: dict[str, Any]) -> None:
        self.send({"jsonrpc": "2.0", "method": method, "params": params})

    def request(self, method: str, params: dict[str, Any], timeout: float) -> dict[str, Any]:
        identifier = self.next_id
        self.next_id += 1
        self.send({"jsonrpc": "2.0", "id": identifier, "method": method, "params": params})
        deadline = time.monotonic() + timeout
        while True:
            message = self.take(deadline)
            if message is None:
                raise TimeoutError(f"{method} timed out")
            if message.get("id") == identifier and "method" not in message:
                if "error" in message:
                    raise RuntimeError(f"{method}: {message['error']}")
                return message.get("result", {})
            self.answer(message)

    def take(self, deadline: float) -> dict[str, Any] | None:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return None
        try:
            return self.incoming.get(timeout=remaining)
        except queue.Empty:
            return None

    def answer(self, message: dict[str, Any]) -> None:
        """Server-to-client requests: an empty answer is enough for every one sourcekit-lsp asks."""
        identifier = message.get("id")
        method = message.get("method")
        if identifier is None or not isinstance(method, str):
            return
        result: Any = None
        if method == "workspace/configuration":
            items = message.get("params", {}).get("items", [])
            result = [None] * len(items)
        self.send({"jsonrpc": "2.0", "id": identifier, "result": result})


def uri_of(path: Path) -> str:
    return path.resolve().as_uri()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout", type=float, default=TIMEOUT_SECONDS)
    parser.add_argument("--settle", type=float, default=SETTLE_SECONDS)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument("files", nargs="*", type=Path)
    args = parser.parse_args()

    root = args.root.resolve()
    files = args.files or sorted(
        path for directory in ("Sources", "Tests", "Package.swift")
        for path in ([root / directory] if directory.endswith(".swift") else (root / directory).rglob("*.swift"))
    )
    files = [path.resolve() for path in files if path.suffix == ".swift"]

    process = subprocess.Popen(
        ["sourcekit-lsp"],
        cwd=root,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    client = Client(process)
    diagnostics: dict[str, list[dict[str, Any]]] = {}
    try:
        client.request(
            "initialize",
            {
                "processId": None,
                "rootUri": root.as_uri(),
                "capabilities": {"textDocument": {"publishDiagnostics": {}}},
                "workspaceFolders": [{"uri": root.as_uri(), "name": root.name}],
            },
            timeout=60,
        )
        client.notify("initialized", {})
        for path in files:
            client.notify(
                "textDocument/didOpen",
                {
                    "textDocument": {
                        "uri": uri_of(path),
                        "languageId": "swift",
                        "version": 1,
                        "text": path.read_text(encoding="utf-8"),
                    }
                },
            )

        deadline = time.monotonic() + args.timeout
        quiet_since: float | None = None
        while True:
            now = time.monotonic()
            if now >= deadline:
                break
            message = client.take(deadline)
            if message is None:
                break
            client.answer(message)
            if message.get("method") != "textDocument/publishDiagnostics":
                continue
            params = message.get("params", {})
            diagnostics[params.get("uri", "")] = params.get("diagnostics", [])
            if len(diagnostics) >= len(files):
                quiet_since = quiet_since or now
                if now - quiet_since >= args.settle:
                    break
            else:
                quiet_since = None
    finally:
        try:
            client.request("shutdown", {}, timeout=10)
            client.notify("exit", {})
        except (TimeoutError, RuntimeError, BrokenPipeError):
            pass
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()

    failures = 0
    for path in files:
        for diagnostic in diagnostics.get(uri_of(path), []):
            severity = SEVERITIES.get(diagnostic.get("severity", 1), "error")
            start = diagnostic.get("range", {}).get("start", {})
            line = int(start.get("line", 0)) + 1
            character = int(start.get("character", 0)) + 1
            message = str(diagnostic.get("message", "")).splitlines()[0]
            print(f"{path}:{line}:{character}: {severity}: {message}")
            failures += severity in FAILING
    missing = [path for path in files if uri_of(path) not in diagnostics]
    if missing:
        print(
            f"sourcekit-lint: {len(missing)} file(s) produced no diagnostics "
            f"(the server may still be building): {', '.join(str(path) for path in missing[:3])}",
            file=sys.stderr,
        )
    return 1 if failures or missing else 0


if __name__ == "__main__":
    raise SystemExit(main())
