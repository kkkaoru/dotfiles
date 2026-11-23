"""H2 and H3, measured on this machine's MLX, on the model's own shapes (hidden 1536, ffn 6144).

H2: decode is bandwidth-bound, so the claim "fewer bits means proportionally faster decode" is a
    bytes claim. This times a quantized matvec at 4 and 2 bits, group 64, batch 1, and reports the
    effective bandwidth of each: if both run at the same GB/s, the time ratio is the byte ratio.

H3: mx.compile fuses elementwise chains into one kernel (MLX docs; measured inference gain in the
    literature is a few percent at batch 1). This times a decoder-shaped chain - rms_norm, quantized
    matvec, activation, quantized matvec - eager against compiled, and reports the CPU/dispatch part
    that a Swift-side rewrite could touch.
"""

import json
import statistics
import time

import mlx.core as mx

HIDDEN, FFN, GROUP = 1536, 6144, 64


def bench(fn, warm=8, runs=40) -> float:
    for _ in range(warm):
        mx.eval(fn())
    samples = []
    for _ in range(runs):
        start = time.perf_counter()
        mx.eval(fn())
        samples.append(time.perf_counter() - start)
    return statistics.median(samples)


def h2() -> list[dict]:
    x = mx.random.normal((1, HIDDEN)).astype(mx.float16)
    rows = []
    for bits in (4, 2):
        w = mx.random.normal((FFN, HIDDEN)).astype(mx.float16)
        wq, scales, biases = mx.quantize(w, group_size=GROUP, bits=bits)
        mx.eval(wq, scales, biases)
        nbytes = wq.nbytes + scales.nbytes + biases.nbytes
        seconds = bench(
            lambda: mx.quantized_matmul(
                x, wq, scales, biases, transpose=True, group_size=GROUP, bits=bits
            )
        )
        rows.append(
            {
                "bits": bits,
                "ms": round(seconds * 1000, 3),
                "MB_read": round(nbytes / 1e6, 2),
                "GB_per_s": round(nbytes / 1e9 / seconds, 1),
            }
        )
    return rows


def h3() -> dict:
    x = mx.random.normal((1, HIDDEN)).astype(mx.float16)
    norm = mx.ones((HIDDEN,), dtype=mx.float16)
    weights = []
    for shape in ((FFN, HIDDEN), (HIDDEN, FFN)):
        w = mx.random.normal(shape).astype(mx.float16)
        weights.append(mx.quantize(w, group_size=GROUP, bits=4))

    def chain(x, norm, up, down):
        up_w, up_s, up_b = up
        down_w, down_s, down_b = down
        h = mx.fast.rms_norm(x, norm, 1e-6)
        h = mx.quantized_matmul(h, up_w, up_s, up_b, transpose=True, group_size=GROUP, bits=4)
        h = h * mx.sigmoid(h)
        h = mx.quantized_matmul(h, down_w, down_s, down_b, transpose=True, group_size=GROUP, bits=4)
        return h

    eager = bench(lambda: chain(x, norm, weights[0], weights[1]))
    compiled = mx.compile(chain)
    fused = bench(lambda: compiled(x, norm, weights[0], weights[1]))
    return {
        "eager_ms": round(eager * 1000, 3),
        "compiled_ms": round(fused * 1000, 3),
        "speedup": round(eager / fused, 3),
    }


def main() -> None:
    print(json.dumps({"h2_bits": h2(), "h3_compile": h3()}, indent=2))


if __name__ == "__main__":
    main()
