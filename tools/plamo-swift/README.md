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
`debug`-mode load (0.055 s) but still an order of magnitude below Python's first request.

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
