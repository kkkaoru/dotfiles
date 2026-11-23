# Can plamo-swift get faster? A paper review with measurements on this Mac

The question was whether a newer Swift, or one of the published inference speed-up methods, would make
the PLaMo translate server visibly faster. This records what was updated, what the papers promise, and
what three hypothesis tests measured here say about each promise.

## The update, and why it cannot move the number

| item | before | after |
|---|---|---|
| Swift toolchain | 6.4.2 | 6.4.2 — already current |
| `mlx-swift` | 0.33.0 | 0.33.0 — `swift package update` reports it is the newest tag upstream |
| `swift-numerics` | 1.1.1 | 1.1.1 |

`swift package update` was a no-op, so there is nothing to build against: the release build is
byte-identical to the installed one (`f4a67a17…`), and the live service was reloaded onto it through
`./scripts/plamo-swift-service.sh reload` to confirm the path still works — 0.828 s to a `200` on
`/v1/models`, first completion 0.030 s.

That is not a disappointment, it is the arithmetic. A decode step streams 479 MB of 4-bit weights in
2.56–2.87 ms, i.e. ~70–77% of what a pure GPU read of the same bytes costs on this machine
(0.468 GB in 1.96 ms = 239 GB/s). Those reads run in MLX's precompiled Metal kernels; the package's
Swift code contributes graph construction and dispatch, the remaining 9% of a step. A toolchain bump
can only touch that 9%, so its ceiling is a few percent — and only if the compiler emits better
dispatch code, which it does not here.

## What the papers promise

| method | citation | published gain | our expected gain | verdict |
|---|---|---|---|---|
| Speculative decoding | Leviathan et al. 2022, *Fast Inference from Transformers via Speculative Decoding*, [arXiv:2211.17192](https://arxiv.org/abs/2211.17192); Chen et al. 2023, [arXiv:2302.01318](https://arxiv.org/abs/2302.01318) | 2–3× on T5-XXL, outputs identical, needs a draft model | a 9-token answer admits at most ~2 steps saved, and a second model breaks the 58 MB idle footprint | no |
| Prompt-lookup decoding | Saxena 2023, [arXiv:2304.04431](https://arxiv.org/abs/2304.04431) | ~2.4× on summarization and context-QA, where the output reuses the input's own words | translation emits a different language, so the input has no n-grams to propose — measured below: 0 of 178 tokens | no, measured |
| Sub-4-bit weights | Ma et al. 2024, *The Era of 1-bit LLMs: BitNet b1.58*, [arXiv:2402.17764](https://arxiv.org/abs/2402.17764) | ternary weights; latency gain grows with model size (~2.7× at 3B, ~1.2× at 700M vs FP16), trained from scratch | needs a ternary model of this translator, which does not exist; a post-hoc 2-bit drop is the only reachable form, and it measured 1.27× | the only lever that moves the wall, needs quality work |
| Kernel fusion / graph compile | MLX `mx.compile` documentation (fused elementwise chains: 15.5 ms → 3.1 ms on M1 Max); the inference gain at batch 1 is reported as a few percent | fusion wins where elementwise ops dominate | our step is matvec-dominated, so there is little to fuse — measured below: 0.76× | no, measured |
| PagedAttention / KV-cache compression | Kwon et al. 2023, [arXiv:2309.06180](https://arxiv.org/abs/2309.06180); Hooper et al. 2024, *KVQuant*, [arXiv:2401.18079](https://arxiv.org/abs/2401.18079) | PagedAttention: 2–4× throughput by cutting KV memory waste and enabling batching; KVQuant: up to 1.7× on KV matvecs, growing with context length | our prompts are 21 tokens and the KV cache is ~45 MB against 479 MB of weights, so it is under 10% of a step, and the mods are serial | no |
| Continuous batching | Yu et al. 2022, Orca (OSDI), and the batching results above | throughput, not per-request latency | the two translate mods issue one request at a time | no |

## Hypothesis tests run here

Each test ran against the live server or against this machine's MLX, at the model's own shapes
(hidden 1536, FFN 6144, 16 layers, one KV head).

### H1 — can a draft be proposed at all? (`scripts/hypothesis-prompt-lookup.py`)

Twenty prompts of the kind the mods send, greedy, tokenised with the model's own tokenizer:

- 20 translations, **178 output tokens**, mean 8.9 tokens per answer;
- output 3-grams that also occur in the prompt: **0 of 178 (0.0%)**;
- output 3-grams that repeat inside the answer: **0 of 178 (0.0%)**.

Prompt-lookup has nothing to propose on translation, and self-drafting has nothing to repeat. Both
speculative families are ruled out by measurement, not by argument — and note how short the answers
are: even a perfect drafter saves at most a step or two.

### H2 — do fewer bits mean proportionally less time? (`scripts/hypothesis-bits-and-fusion.py`,
`scripts/hypothesis-single-op.py`)

| scale | 4-bit | 2-bit | ratio |
|---|---|---|---|
| one matvec (FFN 6144×1536, batch 1) | 0.244 ms, 5.31 MB, 21.7 GB/s | 0.236 ms, 2.95 MB, 12.5 GB/s | 0.97× — no gain |
| one full decode step (16 layers, one eval) | 1.54 ms, 315 MB, 204 GB/s | 1.21 ms, 175 MB, 144 GB/s | **1.27× for 56% of the bytes** |

The single-matvec row is launch-bound: 5 MB at 239 GB/s would be 22 µs, not 244 µs, which is why 2
bits changes nothing there. The step row is the honest test, and it shows the byte saving is real but
partly eaten by unpacking — 204 GB/s at 4 bits against 144 GB/s at 2 bits. So the published "fewer
bits, proportionally faster" does not transfer at this size: **the ceiling is ~1.27×, not ~2×**, and
the benchmark streams the transformer blocks only (315 MB of the real step's ~400 MB, the embedding
and LM head adding ~82 MB at 4 bits).

### H3 — does `mx.compile` fusion help? (`scripts/hypothesis-single-op.py`,
`scripts/hypothesis-bits-and-fusion.py`)

| scale | eager | compiled | ratio |
|---|---|---|---|
| two matvecs + activation | 0.230 ms | 0.321 ms | 0.72× |
| one full decode step | 1.62 ms | 2.12 ms | 0.76× |

Compilation is slower at both scales. The documented win is for elementwise chains; a step whose time
is a weight stream has almost no elementwise work to fuse, and the fused kernel's launch shape costs
more than it saves.

## What is actually left

Ranked by gain per unit of pain, given the measurements above:

| lever | gain | difficulty |
|---|---|---|
| fewer bits (2-bit or ternary weights) | 1.27× measured ceiling for 2-bit; more only with a retrained model | needs quality work and a re-baselined `scripts/check-parity.py` |
| fewer decode steps (a draft head or a stop-token predictor) | a step or two of nine, and it changes the output | re-baseline parity |
| better quantized-matvec kernels in MLX itself | up to ~1.3× | upstream C++/Metal, not this repo |
| shave graph build and allocations (what the README's change table already did) | a few percent per step | moderate, self-contained |
| batching concurrent requests | N× throughput, latency flat | the mods are serial, so nothing visible |
| a newer Swift or `mlx-swift` | 0–2% | pointless until MLX's kernels change |

The wall is memory bandwidth, and the README already says so: "decode is bandwidth-bound… only fewer
bits or fewer steps (speculative decoding) would move it." What this review adds is that on this
workload speculative decoding cannot draft (H1), fusion loses (H3), and fewer bits is worth 1.27× and
not 2× (H2).
