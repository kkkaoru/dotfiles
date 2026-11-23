# Beyond MLX's default path: what was measured, and what lost

The question was whether CoreML, MPSGraph, hand-written Metal or a low-level OS memory trick makes
`plamo-swift` faster or smaller than MLX's own path on this Mac (M5 Pro). Every candidate below was
run here; none of them won, and this page records the numbers so the next attempt starts from them.

Model: `plamo-3-translate-0.8b-mlx-4bit`, 479 MB of 4-bit weights, 16 layers, hidden 1536, FFN 6144,
2048/128 attention heads, 21-token prompts and ~9-token answers.

## Where the time goes

| path | file | measured |
|---|---|---|
| one decode step: graph build + `eval` per token | `Sources/PlamoCore/Model.swift` (`generate`) | 0.24–0.31 ms build, 2.56–2.87 ms GPU |
| `lm_head`, streamed every step | `Model.swift` | 82 MB of the 479 MB (17%) |
| attention, KV concatenated per layer per step | `Model.swift` | ~45 MB at 722 tokens (<10%) |
| reload: `mmap` + adopt, no copy | `Sources/PlamoCore/Packed.swift:133` | 2.4 ms; pages fault in during the first eval |
| Metal buffer cache cap | `Sources/PlamoCore/ModelRuntime.swift` | 32 MB, cleared when going lukewarm |

A step streams 479 MB in 2.56–2.87 ms, and a synthetic 16-layer step of the same shapes runs at
204–280 GB/s across runs — the same order as a pure read of those bytes. That is why nothing below
wins: the kernel is already waiting on memory, not on arithmetic, dispatch or allocation.

## 1. Hand-written Metal 4-bit GEMV — loses

`/tmp/metal_matvec.swift`: one simdgroup per output row, lanes striding the row, `simd_sum` to
reduce; a second variant loading `uint4` instead of `uint32`. Checked against a CPU reference first
(worst relative error 4.4e-4), then timed over the model's own seven matrices × 16 layers, 315 MB:

| kernel | per step | bytes/s |
|---|---|---|
| MLX `quantized_matmul` | 1.54 ms | 204 GB/s |
| hand-written, `uint4` loads | 1.62 ms | 194 GB/s |
| hand-written, `uint32` loads | 1.68 ms | 187 GB/s |

MLX's own quantized matvec is already better tuned than a hand-written one. Writing our own would be
a 5–9% regression for a lot of code.

## 2. fp16 weights for MPSGraph or CoreML — loses by 3.9×

Neither MPSGraph nor CoreML has a 4-bit weight format, so either path must hold the weights as fp16
(or palettize them into a CoreML-specific format). Same 16-layer step, same GPU, weights fp16:

| weights | per step | streamed | bytes/s |
|---|---|---|---|
| 4-bit | 1.12 ms | 315 MB | 280 GB/s |
| fp16 | 4.37 ms | 1120 MB | 256 GB/s |

3.9× slower, and the bandwidth is the same — the extra time is the extra bytes. On top of that
`coremltools` and `torch` are not installed here, `plamo3` is not an architecture `coremltools`
converts, and MLX already dispatches to Metal, so MPS has nothing to add that MLX is not already
using. There is no version of this path worth building.

## 3. `madvise(MADV_WILLNEED)` after the packed `mmap` — no gain, reverted

In isolation the hint looks like a big win: first touch of a 502 MB mapping cost 12.9–15.3 ms of page
faults without it and 0.28 ms with it, in both run orders (`/tmp/madvise_test.c`).

In the server it does nothing, because those faults already happen inside the warm-up's first `eval`,
under GPU work:

| build | cold start, median of 20 fresh processes |
|---|---|
| before | 0.076 s (min 0.073) |
| with `MADV_WILLNEED` after `mmap` | 0.100 s (min 0.094) |
| reverted | 0.092 s (min 0.076) |

No gain, possibly a small loss, so `Packed.load` is unchanged.

## 4. MLX Metal buffer cache limit — no effect on peak or speed

Three builds at 8, 32 and 128 MB, one warm-up run each, then four alternating rounds per build so no
config inherits a warm page cache or a busy GPU:

| cache limit | peak physical | max RSS | 500-token prefill | decode step |
|---|---|---|---|---|
| 8 MB | 1216 MB | 79 MB | 37.1 ms | 2.67 ms |
| 32 MB (shipped) | 1216 MB | 78 MB | 36.8 ms | 2.79 ms |
| 128 MB | 1216 MB | 78 MB | 37.4 ms | 2.71 ms |

Identical within noise. The 1216 MB peak is the 500-token prefill's own activations and mask tables,
not retained buffers, so the cache limit is not the memory lever it looks like. The shipped 32 MB
stays: it is enough for one step's buffers and keeps the cache from growing after a burst.

## 5. Purgeable buffers and reclaimable pages

The packed cache is a read-only file mapping, so its pages are already reclaimable by the kernel —
that is why an idle server sits at ~58 MB of physical footprint while the weights stay addressable.
Making the Metal buffers themselves purgeable is not exposed by MLX Swift, and the buffers a decode
step needs are live for the whole step, so there is nothing to purge mid-request.

## Conclusion

MLX's default path is at the memory wall for this workload. The only lever that moves decode is
fewer bytes per step, which means fewer bits: the earlier 2-bit test measured 1.27× (not 2×) because
unpacking costs part of the byte saving, and it needs a 2-bit model and a re-baselined
`scripts/check-parity.py` before it could ship. Everything else — hand-written Metal, MPSGraph,
CoreML, `madvise`, cache limits — is measured above and loses.

Not done: no 2-bit model was built, no CoreML model was converted (blocked as described), and
batching was not tested because the server's workload is one request at a time.
