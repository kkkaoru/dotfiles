"""H2 and H3 at the scale that matters: one whole decode step, one eval, like the decoder.

A single matvec (5.3 MB) takes 0.24 ms on this GPU - about 22 GB/s, i.e. launch-bound, and 2 bits
buys nothing there. The decoder does not run one matvec per eval; it runs the whole step's ~112
matvecs in one graph, streaming every weight once. So this builds the model's real per-step weight
set (hidden 1536, ffn 6144, 16 layers, GQA with one KV head) at 4 and 2 bits and times one full step.

H2: is a 2-bit step ~half the time of a 4-bit step? That is the bandwidth claim, and it is the only
    lever that moves the wall.
H3: does mx.compile (kernel fusion) beat the eager graph at this scale? The literature's inference
    gain is a few percent at batch 1.

Weight shapes follow MLX's convention here: (out_features, in_features), used with transpose=True.
"""

import json
import statistics
import time

import mlx.core as mx

HIDDEN, FFN, LAYERS, GROUP, Q_DIM, KV_DIM = 1536, 6144, 16, 64, 2048, 128
SHAPES = [
    (Q_DIM, HIDDEN),   # q
    (KV_DIM, HIDDEN),  # k
    (KV_DIM, HIDDEN),  # v
    (HIDDEN, Q_DIM),   # o
    (FFN, HIDDEN),     # up
    (FFN, HIDDEN),     # gate
    (HIDDEN, FFN),     # down
]


def bench(fn, warm=3, runs=15) -> float:
    for _ in range(warm):
        mx.eval(fn())
    samples = []
    for _ in range(runs):
        start = time.perf_counter()
        mx.eval(fn())
        samples.append(time.perf_counter() - start)
    return statistics.median(samples)


def build(bits: int) -> tuple[list, int]:
    layer = []
    for shape in SHAPES:
        w = mx.random.normal(shape).astype(mx.float16)
        layer.append(mx.quantize(w, group_size=GROUP, bits=bits))
        del w
    mx.eval(*[part for triple in layer for part in triple])
    nbytes = sum(t[0].nbytes + t[1].nbytes + t[2].nbytes for t in layer) * LAYERS
    return layer, nbytes


def mm(x, triple, bits: int):
    w, scales, biases = triple
    return mx.quantized_matmul(x, w, scales, biases, transpose=True, group_size=GROUP, bits=bits)


def step(x, layer, bits: int):
    """The layer's matvecs, chained, applied LAYERS times; attention is negligible at 21 tokens."""
    for _ in range(LAYERS):
        q = mm(x, layer[0], bits)
        k = mm(x, layer[1], bits)  # one KV head: kept so the step streams its bytes too
        v = mm(x, layer[2], bits)
        o = mm(q, layer[3], bits)
        up = mm(x, layer[4], bits)
        gate = mm(x, layer[5], bits)
        down = mm(up * mx.sigmoid(gate), layer[6], bits)
        x = x + o + down
    return x


def h2() -> list[dict]:
    x = mx.random.normal((1, HIDDEN)).astype(mx.float16)
    rows = []
    for bits in (4, 2):
        layer, nbytes = build(bits)
        seconds = bench(lambda: step(x, layer, bits))
        rows.append(
            {
                "bits": bits,
                "ms_per_step": round(seconds * 1000, 2),
                "MB_streamed": round(nbytes / 1e6, 1),
                "GB_per_s": round(nbytes / 1e9 / seconds, 1),
            }
        )
        del layer
    return rows


def h3() -> dict:
    x = mx.random.normal((1, HIDDEN)).astype(mx.float16)
    layer, _ = build(4)
    eager = bench(lambda: step(x, layer, 4))
    compiled = mx.compile(lambda x: step(x, layer, 4))
    fused = bench(lambda: compiled(x))
    return {
        "eager_ms": round(eager * 1000, 2),
        "compiled_ms": round(fused * 1000, 2),
        "speedup": round(eager / fused, 3),
    }


def main() -> None:
    print(json.dumps({"h2_bits": h2(), "h3_compile": h3()}, indent=2))


if __name__ == "__main__":
    main()
