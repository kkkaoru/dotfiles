# plamo-swift: what actually limits it

Two investigation passes over `tools/plamo-swift`, the MLX Swift server behind the PLaMo 3 Translate
mods. Every number below was measured on this Mac (M5 Pro) with the shipped
`plamo-3-translate-0.8b-mlx-4bit` checkpoint: 16 layers, hidden 1536, FFN 6144, 16 query heads and
**1 KV head** of 128 dims, vocab 107520, sliding window 2048. The workload is one request at a time,
a ~21-token prompt and a ~9-token answer.

The question each pass asked: is anything faster or smaller than MLX's default path? Pass one tried
other kernels and platforms. Pass two went down to the per-token overheads and asked the literature
what effect sizes are achievable at all.

## 1. Where the time goes

| quantity | measured | where |
|---|---|---|
| decode step, GPU | 2.56–2.87 ms | `Sources/PlamoCore/Model.swift` (`generate`) |
| decode step, graph build | 0.24–0.31 ms | same |
| bytes streamed per step | 479 MB | 4-bit weights, 16 layers |
| achieved bandwidth | 204–280 GB/s | a synthetic step of the same shapes reaches the same figure |
| `lm_head` alone | 92.9 MB, 0.497 ms | `Model.swift`, streamed every step |
| KV cache | 4 KB per token | MQA: 1 head × 128 dims × 2 B × 16 layers |
| KV concatenation at 512 cached tokens | 0.132 ms | `attention`, one `concatenated` per layer per step |
| prefill, 500 tokens | 36.8–37.4 ms | `ModelRuntime.timePrefill` |
| cold start to first completion | 0.076–0.092 s | `scripts/plamo-swift-service.sh reload` |

A step streams 479 MB in 2.6 ms, so the kernel is waiting on memory, not arithmetic. That single
fact explains almost every result below: nothing helps unless it moves fewer bytes per token.

The first pass had estimated the KV concatenation at ~45 MB per step from the hidden size. The
config says `num_key_value_heads: 1`, so the real figure is 4 KB per token — the concatenation is
0.132 ms of launch overhead, not a bandwidth cost. Measuring it before optimising it was the
difference between a plan and a fact.

## 2. What shipped

**Chained decode (`decodeChunk = 2`).** `generate` used to read every token back to the host
(`argMax(...).item()`), which serialises the CPU and the GPU once per step. It now keeps the token as
an `MLXArray`, feeds it into the next step through `logits(token:)`, and reads two steps at once,
truncating the chunk at a stop token. Greedy output is unchanged by construction, and the text of
all 20 test prompts was identical byte for byte.

| graph carries | per completion (20 requests) | per completion (60 interleaved requests) |
|---|---|---|
| 1 step (a sync per token) | 0.0382 s | 0.0531 s |
| **2 steps** | **0.0359 s** | **0.0504 s** |
| 4 steps | 0.0417 s | — |
| 8 steps | 0.0499 s | — |

1.053× on the interleaved run. Four and eight steps per graph are *slower* than a sync per token:
a pending step keeps its activations and its KV copy alive, and past two steps the working set no
longer fits the 32 MB Metal buffer cache, so MLX allocates instead of reusing. A synthetic
measurement had promised 1.16× for eight steps; end to end it delivered 0.69×. The synthetic test
held only the body matrices, not the whole step, which is why its buffer pressure was wrong.

Two tests guard the change: `generateDropsTheRestOfAChunkAtAStopToken` (a stop token in the first
half of a chunk must not leak the second half into the reply) and
`aTokenArrayProducesTheSameLogitsAsTheIDsForm` (the array entry point is the same pass as the ids
form).

Output is unchanged. `scripts/check-parity.py` matches `mlx_lm` on four of its five prompts and
differs on `mixed` by one word — `memory` against Python's `Memory`. Rebuilt from `HEAD` without
this change, the same prompt also says `memory`, so the difference is a greedy near-tie between the
two implementations, not a regression from chaining. The README's "all five match" claim was stale
and now states the actual result.

## 3. Candidates measured and rejected

| candidate | mechanism | measured | verdict |
|---|---|---|---|
| hand-written Metal 4-bit GEMV, one simdgroup per row, `uint4`/`uint32` loads | fewer bytes, better kernel | 1.62–1.68 ms per 315 MB step vs MLX's 1.54 ms (194/187 vs 204 GB/s); correctness 4.4e-4 against a CPU reference | MLX's `quantized_matmul` is already better tuned |
| fp16 weights for MPSGraph or CoreML | another runtime | 4.37 ms, 1120 MB vs 1.12 ms, 315 MB at 4 bits (**3.9×**), same bandwidth | loses on bytes before anything else; plus no 4-bit format in either runtime, `coremltools`/`torch` not installed, `plamo3` not a convertible architecture |
| `madvise(MADV_WILLNEED)` after the packed `mmap` | page-cache prefetch | isolated: 12.9–15.3 ms of first-touch faults → 0.28 ms; in the server: cold start 0.076 / 0.100 / 0.092 s (before / with / reverted) | no gain, reverted |
| MLX `cacheLimit` 8 / 32 / 128 MB, 4 alternating rounds each | allocator | peak 1216 MB, maxRSS 78–79 MB, prefill 36.8–37.4 ms, decode 2.67–2.79 ms — identical | the peak is the prefill's own activations; 32 MB stays |
| purgeable / reclaimable buffers | release pages while idle | the packed mapping is already read-only file cache; an idle server sits at 4.6 MB phys | nothing to add |
| KV preallocation instead of per-step concatenation | avoid the copy | 0.142 ms preallocated vs 0.132 ms concatenated at 512 tokens; 32 KB copied at 64 tokens | the concatenation is already cheaper; the cost is launch overhead |
| 2-bit `lm_head` only | 92.9 MB → 51.6 MB per step | 0.369 ms vs 0.497 ms (1.35×) on that matvec, 139.9 GB/s because unpacking eats part of the saving | ~5% of a step for a quality risk on the output distribution |
| 2-bit whole model (earlier test) | halve the bytes | 1.27× against a 2× ceiling | needs a 2-bit checkpoint and a re-baselined `scripts/check-parity.py` |
| masks | avoid a rebuild per step | read the code: built once per prefill in `attention` | nothing to fix |
| background QoS (`taskpolicy -b`, the low-level-OS lever) | yield CPU and I/O to foreground work | 0.0827 s vs 0.0493 s per completion (30 requests each, 3 alternating rounds) — **1.68× slower** | the agent keeps default scheduling; decode is GPU-bound, so deprioritising it delays every step |
| completion cache | skip identical work | an identical repeat request is answered in 0.7 ms | already present; it also means latency must be measured with distinct prompts |

## 4. Literature review and meta-analysis

The question this pass had to answer with evidence rather than intuition: how large a gain is
achievable at all for a 0.8B model, batch size 1, 4-bit weights, on a machine already at the memory
wall?

**Included.** Papers and measurement reports that give a number for weight-only low-bit decoding at
batch size 1, plus the kernel-level analyses that explain it: GPTQ/OPTQ (Frantar et al., 2023), AWQ
(Lin et al., 2024), Marlin (Frantar et al., 2025), Atom (MLSys 2024), QUIK (EMNLP 2024), DecDEC
(2024), a 2026 meta-review of quantized mid-size serving, two roofline analyses of W4A16 kernels, the
ParetoQ scaling study of 1–4 bit, a faithfulness audit of GGUF precisions, and the speculative
decoding line (SpecDec, staged speculative decoding, ML-SpecQD).

**Pooled effect sizes at batch size 1** (each source against its own BF16 baseline):

| technique | reported speed-up | conditions |
|---|---|---|
| W4A16 vs BF16 | 1.35–2.12× | 3 engines, 3 GPUs; 2.12× Qwen2.5-14B on A100, 1.35–1.37× Gemma 3 12B on H100 |
| W8A16 / FP8 vs BF16 | 1.50–2.06× | same sources |
| W4A16 at batch 256 | 1.01–1.02× | the gain disappears once the matmul is compute-bound |
| Marlin kernel ceiling | 3.87× | 4.125 effective bits, no attention or KV in the figure |
| speculative decoding | 1.36–5× | needs a draft model and a high acceptance rate; 3.16× for GPT-2-L with a 40M draft on an RTX 4090 |
| 2-bit quantization | 2× byte ceiling | quality: 3.4-point average drop for the best QAT, 9.1–10.5 for PTQ/AQLM |

**Why the spread is what it is.** Every source reduces to the same two-term model: the step time is
the larger of the weight stream (bytes ÷ bandwidth) and the compute term, which for W4A16 includes
dequantisation. Arithmetic intensity at decode is ≈ 2/bytes-per-weight — about 4 FLOP/byte at 4 bits
against a ridge point of 156 (A100) or 295 (H100) — so batch-1 decode sits deep under the roof and
time is bytes. That predicts 4× for a quarter of the bytes, and the record shows 1.35–2.12× because
attention, KV traffic, dequantisation and launch overhead do not shrink. The kernel measurements say
the same thing from below: at M=1 a 4-bit GEMV executes as many instructions as the BF16 one
(15.3 M against 15.1 M) while moving a quarter of the bytes, leaving the ALU pipe 64.5% busy and
reaching only 68% of the roof (169 GB/s) against cuBLAS BF16's 92% (228 GB/s).

**Applying it here.** The shipped model is already 4-bit, so the 1.35–2.12× is already spent. What is
left, ranked by the literature's own effect sizes:

| remaining lever | literature effect | what this machine measured | conclusion |
|---|---|---|---|
| 2-bit weights | 2× ceiling, 1.27× realised | `lm_head` 1.35× on its matvec; whole model 1.27× earlier | real but small; quality cost needs a parity baseline |
| batching | 4-bit advantage gone by M ≈ 28–39 | workload is one request at a time | would not help even if a queue existed |
| speculative decoding | 1.36–5× | 9-token answers, no draft model in the repo | a new model dependency, excluded by the task; a 0.8B self-draft has nothing to offer |
| KV quantisation | helps long context | KV is 4 KB/token, 0.132 ms per step | irrelevant at this shape |
| fewer host round trips | not in the literature | 1.053× end to end | **shipped** |

The meta-analysis therefore agrees with the measurement: at 204–280 GB/s on 479 MB per step this
server is at the memory wall, and the only lever left with a published effect is fewer bits.

## 5. Not done, and why

- **No 2-bit checkpoint.** It is the one remaining lever with a real effect, but it needs a converted
  model plus a re-baselined `scripts/check-parity.py`; the earlier 1.27× figure came from synthetic
  weights and translation quality at 2 bits is unmeasured.
- **No CoreML conversion.** Neither CoreML nor MPSGraph has a 4-bit weight format, `coremltools` is
  not installed, and `plamo3` is not an architecture it converts.
- **No speculative decoding.** It needs a second model in the repo and a tuned acceptance rate; with
  a 9-token answer the fixed cost of drafting would eat most of the win.
- **No batching.** The mods issue one request at a time, and the roofline says the 4-bit advantage
  ends around 28–39 concurrent rows anyway.
- **No QoS change.** Measured, not assumed: running the same binary under `taskpolicy -b` costs
  1.68× per completion, so the launchd agent keeps default scheduling.
- **No residency work.** The packed mapping's pages are already reclaimable file cache, which is why
  an idle server holds 4.6 MB; nothing there needs a knob.

## 6. Reproducing

```sh
cd tools/plamo-swift
bash scripts/check.sh                                   # format, lint, sourcekit, build, tests, coverage
swift build -c release
./scripts/plamo-swift-service.sh reload                 # install and restart the agent
curl -s http://127.0.0.1:18555/v1/models                # 200 when serving
```

Latency must be measured with **distinct prompts**: the server caches identical completions and
answers a repeat in 0.7 ms. The figures in section 2 come from serving two builds on separate ports
and alternating ten distinct translation prompts, 60 requests per build.
