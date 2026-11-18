#!/usr/bin/env python3
"""mlx_lm.server with a listen backlog that survives the mods' burst of requests.

Eight translations can arrive at once (tools/*-plamo-translate). CPython's ThreadingHTTPServer keeps
`request_queue_size = 5`, so the kernel drops the extra connections and the client sees
`ECONNRESET`; a dropped request leaves that line untranslated. Everything else is mlx_lm.server's
own behaviour: measured on an M5 Pro, PFN's serve.py defaults (shorter prefill steps, one prompt
cache) were no faster for these short prompts.

Run: plamo-mlx-server --model <mlx-dir> --port 18555 --trust-remote-code
"""

import http.server
import sys

http.server.ThreadingHTTPServer.request_queue_size = 512

from mlx_lm.server import main  # noqa: E402 - after the class attribute is set

if __name__ == "__main__":
    sys.exit(main())
