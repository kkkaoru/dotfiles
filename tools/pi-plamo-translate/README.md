# pi-plamo-translate

A pi extension that talks to the same local
[PLaMo 3 Translate 0.8B](https://huggingface.co/pfnet/plamo-3-translate-0.8b) server as
`tools/claude-plamo-translate`. Nothing leaves the machine.

You see Japanese. The agent reads and writes English.

- A prompt you type in Japanese is translated to `promptLanguage` (default `en`) before the model
  reads it. The transcript shows what you typed. Slash commands, `!` shell lines, and messages
  injected by extensions are not rewritten.
- One text may cost at most 128 requests: two lines per request, so a text longer than 256 lines
  is left as written. Drawing never starts a translation either, so a redraw of a long transcript,
  a resume or a reload asks the model for nothing.
- Assistant text, and compaction summaries, are drawn in `replyLanguage` (default `ja`). The session
  keeps the English. Translations survive `/reload`, `/resume` and `/fork` because the process keeps
  them, so a redraw shows the same Japanese again and asks the server for nothing. A text never
  translated before is translated only when it fits the newest 120 untranslated lines of the
  transcript and is at most 256 lines, so a reload or a resume redraws about three screens of
  Japanese and opening a long session cannot flood the translation server. Tool rows and tool
  output stay as written.
  While a reply is still streaming it is shown as written; the Japanese replaces it when the
  translation lands. Prose is translated one paragraph per request, and the lines a hard-wrapped
  paragraph was split into are joined first, so a sentence is never translated in halves.
- Code spans, fenced blocks, URLs and the inside of 「...」 are not sent for translation. If the
  server is down, the original text is used and a warning is shown once.
- A translation still running after two seconds reports itself in the footer (`PLaMo 12/30`),
  updated once every five seconds and cleared when the run ends. The count is short on purpose: the
  footer line is shared with the other extensions' statuses.

A prompt recalled with the up arrow in the current session is still what you typed. After a reload,
editor history is the English stored in the session.

## Install

The model is set up once per machine (it is not in Git):

```sh
./scripts/setup-plamo-translate.sh
```

Other macOS versions — 15, 14, 13 and older, and Intel Macs — with the floors that decide which
server runs: [docs/plamo-translate-macos-versions.md](../../docs/plamo-translate-macos-versions.md).

On a Mac that downloads `kkk4oru/plamo-3-translate-0.8b-mlx-4bit-fast`, an all-4-bit conversion of the
model: it reads 478 MB per decode step where PFN's own MLX-4bit release reads about 0.98 GB, so it is
roughly twice as fast, at a small accuracy cost. `scripts/bench-plamo-translate.py --port 18555`
measures it on a fixed sample set.

`./create-symlinks.sh` links this directory to `~/.pi/agent/packages/pi-plamo-translate`. Settings
load `./packages/pi-plamo-translate`. Start a new session, or run `/reload`.

Options live under `pi-plamo-translate` in `~/.pi/agent/settings.json`:

| Option           | Default | Meaning                                                               |
| ---------------- | ------- | --------------------------------------------------------------------- |
| `replyLanguage`  | `ja`    | Language replies and summaries are shown in. `off` disables it.       |
| `promptLanguage` | `en`    | Language your prompts are translated into. `off` disables it.         |
| `port`           | `18555` | Local PLaMo server, started on demand and shared with the Claude mod. |

The server is `~/.local/share/plamo-swift/plamo-swift` when `scripts/setup-plamo-translate.sh`
installed it (MLX
Swift: 0.55 GB resident instead of 1 GB, ready in 0.15 s, identical output), and
`plamo-mlx-server` when it did not. With the Swift server the mod also passes
`--socket ~/.local/share/plamo-swift/plamo.sock` and sends every request over that socket once the
file exists: 23.3 ms per translation against 23.4 ms over loopback TCP on an M5 Pro, so the socket
buys little today — decode dominates — but the transport stops competing with whatever else is on
the network stack. That file can go stale (a second `serve` unlinks the live server's path before it
finds the port taken), and then it refuses while TCP still answers: the mod retries the same request
over the port rather than reading a healthy server as down.

Up to eight translations run at once, a new prompt's first: batching them is what keeps an Apple
Silicon GPU busy. `mlx_lm.server` batches them into one decode loop, so eight translations finish in
about a third of the time eight sequential ones take (measured: 107 ms vs 305 ms), at a per-request
latency of 70-100 ms instead of ~40 ms. `scripts/bench-plamo-translate.py --concurrency 8` measures
it on a fixed sample set.

Greedy completions are also cached on disk (`~/.cache/plamo-translate/`), keyed by the prompt, the
token limit, the stop sequence and the model's `config.json`. The mods re-translate the same text
constantly — a redrawn block, a scrolled screen, a resumed session — and a repeat is answered
byte-identically in under a millisecond instead of ~100 ms per line (a three-line block: 158 ms,
then 1.2 ms). Sampled requests and `"cache": false` bypass it; `GET /cache/stats` reports the hit
rate.

Starting the server is only half of a cold start, and the other half is not what it looks like. MLX
ships its Metal kernels already compiled (`mlx/lib/mlx.metallib` in the wheel): there is no on-disk
kernel cache, no environment variable to point at one, and nothing to pre-build. Measured in fresh
processes: `import mlx` 0.06 s, a first matmul 0.18 s then 0.01 s (only the page cache), and once
the weights are loaded a completion costs 0.11 s — the same as the next one. What the first
completion really pays is `mlx_lm.server` loading the model inside its generation thread, on that
request, while `/v1/models` already answered. So `scripts/plamo-mlx-server.py --plamo-prebuild`
loads the model and answers one completion once, at setup, leaving the weights, the metallib and the
completion cache in the page cache — kernel-owned, so it survives the idle exit and the next cold
start reuses it (measured: 9.5 s cold, 8.2 s with it warm). The mods themselves send one throwaway
completion once the server answers — but only when nothing is waiting: a translation that has
already arrived pays the same load itself and returns its answer, so warming ahead of it would just
add that wait. Readiness is polled every 50 ms. The launch starts as soon as pi loads this package —
before `session_start`, which waits for pi to restore the session — so the wait passes while pi is
still starting up and you are still typing. A warm-up that fails is ignored: the first translation
then simply pays the cost itself.

The model is ~1 GB resident, so the server does not stay: it exits after `PLAMO_IDLE_MINUTES`
(default 10, `0` keeps it) with no translation in flight, and the next translation starts it again.
A `/v1/models` probe does not count as activity: both mods poll it, and counting the poll kept the
process alive forever (measured: still running 100 s after the last translation with
`PLAMO_IDLE_MINUTES=1`). Memory is capped too — MLX frees buffers only up to `mx.set_cache_limit`
(256 MB here) and the prompt cache holds two KV caches — so RSS stays at ~1.03 GB across requests,
8 concurrent ones included, instead of ratcheting up. That lives in `scripts/plamo-mlx-server.py`,
so the Claude mod gets the same behaviour, and nothing starts the server at login or while pi is
not running.

## Check

```sh
bun install
bun run check
```
