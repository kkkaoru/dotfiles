# plamo-swift

PLaMo 3 Translate 0.8B (MLX 4-bit) on [mlx-swift](https://github.com/ml-explore/mlx-swift), with the
same OpenAI-compatible endpoints as the Python `mlx_lm.server` the translate mods use today. It
replaces a ~1 GB Python process with a ~0.55 GB Swift one and cuts launch and first-token latency by
an order of magnitude, at equal steady-state speed and equal output.

## What it is

- `Sources/PlamoCore/Tokenizer.swift` — replays PLaMo 3's Unigram trie; `scripts/dump-tokenizer.py`
  dumps the trie from the model's own `tokenization_plamo.py` so the Swift side only walks it.
- `Sources/PlamoCore/Model.swift` — the plamo3 decoder: quantized 4-bit affine (group 64) matmuls,
  the four distinct RMSNorm offsets, GQA with q/k norm before RoPE, sliding-window and full attention
  alternating every 8 layers, KV cache with window trimming.
- `Sources/PlamoCore/Server.swift` — HTTP/1.1 on Network.framework, keep-alive included: one
  listener on `127.0.0.1:<port>` and, with `--socket`, a second on a Unix socket. Requests are
  served concurrently on a task group, so a second translation never waits for the first.
- `Sources/plamo-swift/main.swift` — `tokenize`, `load`, `debug`, `generate`, `serve`; the server
  answers `GET /v1/models`, `POST /v1/completions` and `POST /v1/chat/completions`, and exits after
  `--idle-seconds` without a completion, exactly like `scripts/plamo-mlx-server.py`.
- `Tests/PlamoCoreTests/` — tokenizer, options, HTTP parsing and the router's status codes
  (`swift test`, 18 tests).

## Parity

`scripts/check-parity.py <model> .build/release/plamo-swift <tokenizer blob>` compares greedy output
with `mlx_lm.generate` on a fixed prompt set (ja→en, en→ja, a long English sentence, code, mixed
Japanese/English). All five match token for token:

```
ok   ja->en: "Hello, world."
ok   en->ja: "Hello, world."
ok   long: "The quick brown fox jumps over the lazy dog and keeps runnin"
ok   code: "def add(a, b):\n    return a + b"
ok   mixed: "I want to speed up inference with MLX. I want to keep memory"
```

Top-5 logits of the 18-token Japanese prompt: Swift `7272 25.36, 44835 19.77, 6238 12.82, 72 11.04`
vs Python `7272 26.38, 44835 19.38, 6238 13.19, 72 11.56` — same order, magnitudes within bfloat16
noise. `scripts/reference-logits.py` and `scripts/reference-layers.py` print the Python side,
`plamo-swift debug` prints the Swift side including per-layer norms, which is how the two divergences
found during the port were located:

- RoPE must use MLX's `base ** (arange(0, dim, 2) / dim)`, not `base ** (i / dim)`.
- mlx_lm's plamo3 ignores the checkpoint's YaRN `rope_parameters` and does not scale embeddings by
  `sqrt(hidden_size)`; the port ignores them too, because matching the running server is the goal.

## Benchmark

`scripts/benchmark.sh both` launches each server on its own port, waits for `/v1/models`, then sends
five distinct 32-token completions (`"cache": false`, so the Python server's greedy-completion cache
cannot answer them) and reports the median of the last four. Measured on this Mac (M5 Pro, macOS
27.2, MLX Swift 0.33.0, mlx-lm from `uv tool`):

| | Python `mlx_lm.server` | Swift `plamo-swift serve` |
| --- | --- | --- |
| launch to `/v1/models` answering | 1.12 s | 0.15 s |
| first completion (32 tokens) | 2.81 s | 0.17 s |
| steady completion (32 tokens, median of 4) | 0.096 s | 0.038 s |
| resident set | 1027 MB | 553 MB |
| physical footprint | 1010 MB | 665 MB |
| compressed memory | 0 B | 0 B |

The first Python completion includes its lazy graph construction and kernel compilation; the Swift
side compiles its Metal kernels during load, which is why its launch number is larger than its
`debug`-mode load (0.055 s) but still an order of magnitude below Python's first request. Those
32-token requests stop at EOS after a few tokens, so that row times a short completion, not 32
decodes; the per-token numbers below are taken from `usage.completion_tokens`.

## Where the time goes

Release build, M5 Pro, macOS 27.2.1, MLX Swift 0.33.0, same prompt at `max_tokens` 1 and 64 so the
fixed part and the per-token part separate:

| step | measurement |
| --- | --- |
| transport, `GET /v1/models` on a kept-alive connection | 0.43 ms |
| prefill, 195-char template prompt (23 tokens) | 0.34 ms graph build + 23.1 ms GPU |
| prefill, 1974-char prompt (~500 tokens) | 0.75 ms graph build + 37.7 ms GPU |
| decode, per step | 0.24–0.31 ms graph build + 2.56–2.87 ms GPU |
| decode, per step at a 722-token context | 0.29 ms graph build + 3.07 ms GPU |
| load, page cache warm | 0.052 s total: 0.023 weights, 0.028 warm-up |
| resident / physical footprint while serving | 560 / 632 MB |
| peak physical footprint during a long prefill | 1052 MB |
| compressed | 0 B |

`plamo-swift bench --model <dir> --prompt <text>` prints these numbers, splitting every forward
pass into the time the CPU spends building the graph and the time the GPU spends running it.

Decode is bandwidth-bound: the 4-bit weights are about 0.5 GB and every step streams them once, so
the effective bandwidth is ~111 GB/s and only fewer bits or fewer steps (speculative decoding) would
move it. It stays flat from 23 to 722 tokens of context, which also rules out the KV concatenation
as the bound: at 722 tokens that copy is ~45 MB per step, under 10% of the 3 ms. Prefill is the
opposite shape — 0.047 ms/token marginal but ~8.6 ms fixed per forward pass — so a short translation
is dominated by graph construction and kernel launches, not by its tokens.

### What was changed for these numbers

| change | before | after |
| --- | --- | --- |
| one mask table per attention kind, built once per pass, instead of one per layer (16 identical `[queries][keys]` allocations) | 500-token prefill: 3.02 ms build, 2.35 ms of it masks | 0.68–0.79 ms build, 0.29–0.32 ms masks |
| `F_RDADVISE` over the 478 MB safetensors before the tokenizer decode, so the read starts early and runs on the disk queue | load 0.088 s (0.031 weights) | 0.052–0.061 s (0.020–0.030 weights) |
| per-step graph build | 0.33 ms | 0.24–0.31 ms |

The cold-load case is the one measurement this machine could not produce honestly: forcing the page
cache out needs root (`purge`), and neither a 24 GB disk write nor 40 GB of anonymous pressure moved
the file-backed page count (`vm_stat` stayed at ~3.7 GB), so every "cold" load still read at 8–19 GB/s
and the numbers were warm ones. The prefetch is in place for the real cold path; it is unmeasured here.

## Shared memory, prefix reuse and the Metal knobs, evaluated

The question was whether the mods should hand the server a shared memory region instead of a JSON
body over a Unix socket, and what else Metal offers. Every row below is a measurement from the table
above or from the research in this section:

| mechanism | saving | cost | verdict |
| --- | --- | --- | --- |
| Mach/POSIX shm, IOSurface or an mmap ring instead of the socket | the transport is 0.43 ms of a 25 ms short request and of a 316 ms long one; the socket already measured 23.3 ms against 23.4 ms for loopback TCP | a second protocol, its framing, an owner for the region, and release handling on both sides, in two clients | not built: under 2% of a short request, 0.1% of a long one |
| KV cache seeded with the shared template prefix | ~20 tokens of the prefix at 0.047 ms/token ≈ 1 ms of 25 ms; the 8.6 ms fixed forward cost stays | cache snapshot, cloning into each request, invalidation on model or template change | rejected |
| greedy completion cache | a repeat costs 0.3–0.8 ms instead of 26.7 ms (short) or 316 ms (long) | an LRU actor keyed by prompt, token cap and stop list; 512 short strings | implemented, see below |
| MLX `Memory.cacheLimit` | already 32 MB instead of the device maximum; the measured peak footprint is prefill activations, not cache | none | kept |
| wired-memory tickets (`WiredMemoryManager`) | none: compressed is 0 B and nothing pages out | pins memory a burst could otherwise return | rejected |
| `asyncEval` overlap | tokenization is under 1 ms of a 25 ms request, and each decode step needs the previous argmax | a second in-flight graph and its memory | rejected |
| mmap the safetensors instead of reading them | load is already 26 ms warm; only a cold page cache would gain | unpacking 4-bit weights through a mapped window | rejected in favour of `F_RDADVISE`, which keeps the read path unchanged and cannot fail a start |
| dropping `Memory.clearCache()` after every completion | 24.8 → 23.6 ms on repeats, inside the noise | up to the 32 MB cache limit stays resident | done: the cache limit bounds it, so the per-request clear was removed |

`CachingProvider` wraps the runtime in `serve`. Every request the router answers is greedy, so the
prompt, the token cap and the stop list are the whole key, and the oldest entry is dropped past 512
entries. A hit returns the stored `Completion` unchanged — the same text, the same token count, the
same `seconds` as when it was produced.

## Building and running

The Metal toolchain is a cryptex that `XcodeDefault` does not resolve, so builds need the toolchain
id from `xcodebuild -downloadComponent MetalToolchain`:

```sh
TOOLCHAINS=com.apple.dt.toolchain.Metal.32023.922.4 swift build -c release
~/.local/share/uv/tools/mlx-lm/bin/python scripts/dump-tokenizer.py \
  ~/.local/share/plamo-translate/plamo-3-translate-0.8b-mlx-4bit /tmp/plamo-tokenizer.bin
.build/release/plamo-swift serve \
  --model ~/.local/share/plamo-translate/plamo-3-translate-0.8b-mlx-4bit \
  --port 18555 --idle-seconds 600 --socket ~/.local/share/plamo-swift/plamo.sock
```

`--tokenizer` defaults to `tokenizer.bin` inside `--model`, which is where
`scripts/setup-plamo-translate.sh` puts the dump.

Both translate mods start `~/.local/share/plamo-swift/plamo-swift` — the binary itself, not the
`~/.local/bin/plamo-swift` wrapper, so a wrapper whose target is missing falls through at once — and
fall back to `plamo-mlx-server`, then to llama.cpp, when the binary is absent. The pi mod also
passes `--socket ~/.local/share/plamo-swift/plamo.sock` and sends its requests over that socket once
the file exists: 23.3 ms per translation against 23.4 ms over loopback TCP on an M5 Pro, because
decode dominates and the round trip is 0.1 ms of it. The socket is there so the transport costs
nothing as the machine gets busier, not because it is the bottleneck today.
