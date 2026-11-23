# plamo-swift

PLaMo 3 Translate 0.8B (MLX 4-bit) on [mlx-swift](https://github.com/ml-explore/mlx-swift), with the
same OpenAI-compatible endpoints as the Python `mlx_lm.server` the translate mods use today. It
replaces a ~1 GB Python process with a ~0.55 GB Swift one and cuts launch and first-token latency by
an order of magnitude, at equal steady-state speed and equal output.

Which server a machine can run, and the setup steps for a macOS version other than this one, are in
[docs/plamo-translate-macos-versions.md](../../docs/plamo-translate-macos-versions.md).

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
  answers `GET /v1/models`, `POST /v1/completions` and `POST /v1/chat/completions`, exits after
  `--idle-seconds` without a completion, exactly like `scripts/plamo-mlx-server.py`, and drops the
  weights into a lukewarm state after `--lukewarm-seconds` (default 60) of the same idle.
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
| launch to `/v1/models` answering (listener first, warm-up in flight) | 0.067 s |
| first completion in a fresh process | 0.372 s |
| resident / physical footprint while serving | 558 / 631 MB |
| resident / physical footprint in the lukewarm state | 78 / 58 MB |
| first request after the lukewarm release | 0.061 s median (0.060–0.097 s of four) |
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
| the warm-up runs the 18-token translation template instead of one token, so the first request finds the prefill shapes and masks already built | first completion 28.6 ms (median of 20 fresh processes) | 24.4 ms |
| the listener starts before the warm-up, so `/v1/models` answers while the first forward pass runs | ready 0.696 s (warm-up finished first) | ready 0.067 s, first completion 0.372 s |
| the lukewarm state drops the weights and the Metal cache after `--lukewarm-seconds` idle | 631 MB phys while serving, held until the process exits | 60 MB phys; the next request reads the weights again in 0.373 s, steady decode unchanged |

### States: cold, warm, lukewarm

| state | what stays resident | footprint |
| --- | --- | --- |
| cold | nothing; no process | 0 |
| warm | weights, tokenizer, compiled kernels, Metal buffer cache, completion cache | 558 MB rss / 631 MB phys |
| lukewarm | tokenizer, compiled kernels, completion cache | 78 MB rss / 58 MB phys |

Measured transitions (M5 Pro, release build; a fresh process per column, from `scripts/soak.py` and
`scripts/measure-lukewarm.py`):

| transition | what happens | measured |
| --- | --- | --- |
| cold → ready | spawn, dyld, MLX init, tokenizer decode, weight read, listener | 0.063 s |
| cold → warm | the above, then the warm-up's first eval compiles the Metal kernels | 0.216 s to the first completion |
| warm → lukewarm | `--lukewarm-seconds` idle (default 60) plus one idle-check tick (≤5 s), then the weights and the Metal buffer cache are dropped | the release is a `nil` and `Memory.clearCache()`; the state is entered within one tick of the window |
| lukewarm → warm | `mmap` of the packed cache, then every tensor adopted as a no-copy Metal buffer over those pages; no spawn, no tokenizer decode, no kernel compile, no read | 2.50–5.29 ms to the reload, 0.05–0.08 s to the first completion |

The reload no longer reads the weights, and it does not allocate them either. `plamo-swift pack
--model <dir>` writes `model.packed` beside the checkpoint once (479.2 MB in 0.31 s): the same
tensors, each starting on its own page boundary, with the checkpoint's size and modification time in
`__metadata__`, so a replaced model is repacked rather than adopted stale. `Packed.load` maps that
file read-only and adopts each slot with `MLXArray(rawPointer:)`, which hands Metal the pointer
without a copy: the weights *are* the page cache. The system can reclaim them under pressure, and the
process never owns a 478 MB anonymous region. The mapping is unmapped by its own `deinit`, i.e. when
the last adopted array dies, because MLX can still hold a Metal buffer over those pages after the
model itself is released.

Measured on the same page-cached 501 MB (`PLAMO_LOAD_BENCH=1 swift test --filter LoadBenchTests`):

| read of the same 501 MB, page-cached | best of 3 |
|---|---|
| `loadArrays` (parse 2.91 ms) + `eval` (11.13 ms) | 14.05 ms |
| packed cache: `mmap` + adopt 2.43 ms, `eval` 0.07 ms | 2.50 ms |
| raw `pread`, 14 threads (no adoption) | 7.34 ms (68.3 GB/s) |

The eval is free because there is nothing left to materialize: the tensors are the mapping. That is
also why more threads win nothing here — a faster copy of bytes that no longer need copying is not a
speed-up. On a spare port the reload is `reload.begin` → `lukewarm.reload` in 2.50–5.29 ms against
0.060–0.097 s for the `loadArrays` path, and the lukewarm footprint is unchanged at 57–59 MB phys
(78 MB rss), because file-backed pages are not the process's own memory. A cold start adopts the
same cache (`reload.packed` at 33.0 ms, `listening` at 34.6 ms from process start), so it too skips
the 11.5 ms of copy that `loadArrays` plus `eval` cost.

The installed service picks this up through `./scripts/plamo-swift-service.sh reload`: it packs the
cache, installs the release build into a fresh inode and re-registers the launchd agent. That takes
0.49 s end to end, and a `/v1/models` probe every 20 ms saw a single refusal in it — `bootout` plus
`bootstrap`, because a `kickstart -k` waits out the plist's `ThrottleInterval 10` (10.6 s measured).
Nothing writes into the file the old process mapped, so the swap cannot break a running image.

A loader that reads the checkpoint itself was tried first and rejected: safetensors header parse, one
page-aligned arena, a parallel `pread`, then `MLXArray(rawPointer:)` adoption. It measured 19.0 ms at
best (26.4 GB/s) against `loadArrays`' 11.4–11.7 ms, and more threads made it worse (25.5 ms at 8,
25.0 ms at 14), because the bytes have to land in memory the loader owns and writing them faults in
~30 600 fresh pages. The packed cache wins by never doing that copy at all.

Two findings from that attempt are worth keeping. A freed 479 MB malloc region stays resident as an
empty large-allocator region (measured 497 MB empty but still dirty, and `malloc_zone_pressure_relief`
does not return it), where `munmap` returns the pages at once — so a design that owns the weights'
memory must map it, not malloc it. And a `deinit` runs before Swift releases a class's stored
properties, so freeing an arena from the model's `deinit` unmaps bytes its arrays still point at; the
packed mapping is therefore owned by the arrays' finalizers, not by the model.

Keeping the weights resident across the release would skip the read too — MLX adopts a foreign pointer
without copying, so a retained 478 MB `MTLBuffer` can be re-adopted with no read and no upload — but
that is a residency trade, not a speed-up: 478 MB held for hours, purgeable only under memory
pressure, which is exactly when the next request needs it back, to save ~14 ms once per idle gap. The
packed cache takes the same win and keeps the 58 MB idle footprint.

### How the lukewarm state works

`ModelRuntime` keeps the model as an optional. `releaseWeights()` sets it to `nil` and calls
`Memory.clearCache()`, which returns the weights and the Metal buffer cache to the system; the next
request's `current()` loads the directory again. The tokenizer, the completion cache and the compiled
kernels stay, so the reload is an `mmap` of the packed cache and nothing else — and with no packed
cache, MLX's own loader reads the checkpoint as before.

`plamo-swift pack --model <dir>` writes that cache once per model (idempotent: a matching size and
mtime stamp in `__metadata__` means no rewrite). Until it has run, the reload falls back to
`loadArrays`; a checkpoint that changes is repacked on the next `pack` rather than adopted stale.

| first request after the lukewarm release | 0.048 s (packed cache) |

Run it once after a model download, e.g. `plamo-swift pack --model ~/.local/share/plamo-translate/
plamo-3-translate-0.8b-mlx-4bit`.

The state is entered by the watcher `serve` already runs for `--idle-seconds`: the same tick now also
releases the weights once per idle stretch, after `--lukewarm-seconds` (default 60, `0` disables) with
no request. `--idle-seconds 0` never exits, and both translate mods pass `--idle-seconds 14400`, so a
session's server holds the lukewarm state instead of paying a spawn and a reload per idle gap.
`PLAMO_TIMING=1` logs `lukewarm.begin`, `lukewarm`, `reload.begin`, `reload.packed` (or
`reload.arrays` when there is no cache) and `lukewarm.reload` with their durations, and
`LoadBenchTests` (gated on `PLAMO_LOAD_BENCH`, needs the checkpoint) compares the packed adoption with
`loadArrays`.

`scripts/measure-lukewarm.py` (port 18558, so a live server keeps serving) measures the states. Every
prompt carries a nonce: a repeated prompt is answered from the completion cache in about a
millisecond and would hide the reload entirely — the first version of this measurement reported
560 MB and a 1 ms reload because of exactly that.

### Soak

`scripts/soak.py` starts its own server on port 18559, prints the three states, runs 30
warm→lukewarm→reload cycles and then 240 continuous requests, and reports rss/phys after each phase.

| phase | first | last | min | max | drift |
| --- | --- | --- | --- | --- | --- |
| warm, after a burst | 633 | 639 | 633 | 641 | +6 MB |
| lukewarm, after the release | 59 | 61 | 59 | 61 | +2 MB |
| continuous, physical | 636 | 636 | 633 | 636 | +0 MB |
| continuous, resident | 563 | 566 | 563 | 566 | +3 MB |

Thirty cycles of warm → release → reload end 6 MB above where they started, inside a 633–641 MB band
that does not trend: the last cycle sits below the maximum, and the 240 continuous requests that
follow are flat to the megabyte (636 MB physical, first and last). The release itself costs 10.58 ms
(`lukewarm.begin` → `lukewarm` in the `PLAMO_TIMING` marks); the reload lands on the first request
after it.

The 6 MB is the Metal buffer cache and the allocator's free pages settling at a working set rather
than at zero — `Memory.clearCache()` returns the weights, not every buffer the allocator keeps for
reuse. A leak would accumulate across the 30 cycles and keep climbing through the continuous phase;
neither happens.

### There is no persistent kernel cache

MLX Swift compiles its Metal kernels at the first eval of each process: 582 ms of the 662 ms warm-up
measured earlier. This version writes no kernel cache to disk, so every new process pays that again.
Lukewarm keeps the compiled kernels in the process, which is why the reload has no compile step — but
that only removes time the warm-up had already overlapped, so the transition table above shows the
reload level with a cold start rather than ahead of it.

A persistent cache would mean an `MTLBinaryArchive` keyed by every kernel variant the model uses,
with a version check on load; the 582 ms it would save is paid once per process, at the first
completion, not per request. That trade is not made here.

The cold-load case is the one measurement this machine could not produce honestly. Forcing the page
cache out needs root (`purge`); without it, neither a 24 GB disk write nor 40 GB of anonymous pressure
moved the file-backed page count (`vm_stat` stayed at ~3.7 GB), so every "cold" load still read the
weights at 8–19 GB/s. A read of the 501 MB file with `F_NOCACHE` set, which bypasses the unified
buffer cache, still measured 34.65 GB/s — this SSD answers a sequential read from its own cache, so
the number says nothing about a cold device read either. What is left is the mechanism: the prefetch
issues `F_RDADVISE` before the tokenizer decode and returns immediately, so a real cold read overlaps
that 20–30 ms of work instead of following it. It is unmeasured here, and no claim is made for it.

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
