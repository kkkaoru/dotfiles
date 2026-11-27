"""H10, measured: what a batched decode step could reach, and why it is not built.

A decode step streams the same weights whatever the row count, so M rows cost t(M) instead of
M x t(1) and the ratio t(1)/t(M) is the throughput ceiling batching could reach. Two measurements:
the shipped FFN matmuls at M = 1..64 on MLX's own 4-bit kernels (group 64), and the real 4-bit
checkpoint's decode loop, where per-row masks, stop checks and cache trimming also scale with M.

    ~/.local/share/uv/tools/mlx-lm/bin/python tools/plamo-swift/scripts/hypothesis-batching.py
"""

import os
import statistics
import sys
import time

import mlx.core as mx

HIDDEN = 1536
FFN = 6144
VOCAB = 107520
QKV = HIDDEN + 2 * 128
LAYERS = 16
GROUP = 64
BITS = 4
SHAPES = {
    "gate_up 12288x1536": (2 * FFN, HIDDEN),
    "down 1536x6144": (HIDDEN, FFN),
}
ROWS = (1, 2, 4, 8, 16, 32, 64)


def quantized(out_features: int, in_features: int):
    weight = mx.random.normal((out_features, in_features)).astype(mx.bfloat16)
    packed, scales, biases = mx.quantize(weight, group_size=GROUP, bits=BITS)
    mx.eval(packed, scales, biases)
    return packed, scales, biases


def matmul(x: mx.array, weights) -> mx.array:
    return mx.quantized_matmul(
        x, weights[0], scales=weights[1], biases=weights[2], group_size=GROUP, bits=BITS
    )


print(f"{'shape':>20} {'M':>3} {'ms':>7} {'GB/s':>6} {'x of M=1':>9}")
for label, (out_features, in_features) in SHAPES.items():
    weights = quantized(out_features, in_features)
    streamed = sum(part.nbytes for part in weights)
    base = None
    for rows in ROWS:
        x = mx.random.normal((rows, in_features)).astype(mx.bfloat16)
        mx.eval(x)
        samples = []
        for _ in range(30):
            start = time.perf_counter()
            mx.eval(matmul(x, weights))
            samples.append((time.perf_counter() - start) * 1_000)
        median = statistics.median(samples[5:])
        base = base or median
        print(
            f"{label:>20} {rows:>3} {median:>7.3f} {streamed / median / 1e6:>6.0f} "
            f"{median / base:>8.2f}x"
        )

# The real loop: the same checkpoint the server serves, one prefill broadcast to M rows, then steps.
MODEL = os.path.expanduser("~/.local/share/plamo-translate/plamo-3-translate-0.8b-mlx-4bit")
if not os.path.exists(MODEL):
    print(f"\nno checkpoint at {MODEL}: matmul ceiling only")
    sys.exit(0)

sys.path.insert(0, MODEL)
from mlx_lm import load  # noqa: E402 - the checkpoint's own model code
from mlx_lm.models.cache import make_prompt_cache  # noqa: E402

PROMPT = (
    "<|plamo:tag|>dataset<|plamo:msg|>translation<|plamo:tag|>input<|plamo:msg|>おはようございます"
    "<|plamo:tag|>output<|plamo:key|>lang<|plamo:val|>ja<|plamo:key|>form<|plamo:val|>polite"
    "<|plamo:msg|>"
)
STEPS = 24

model, tokenizer = load(MODEL, tokenizer_config={"trust_remote_code": True})
ids = mx.array([tokenizer.encode(PROMPT)])
print(f"\nreal checkpoint, {ids.shape[1]}-token prompt, {STEPS} steps per round")
print(f"{'M':>3} {'step ms':>9} {'ms/seq':>8} {'vs M=1':>7}")
base = None
for rows in (1, 2, 4, 8, 16):
    cache = make_prompt_cache(model)
    logits = model(mx.broadcast_to(ids, (rows, ids.shape[1])), cache=cache)
    token = mx.argmax(logits[:, -1:, :], axis=-1)
    mx.eval(token)
    samples = []
    for _ in range(STEPS):
        start = time.perf_counter()
        logits = model(token, cache=cache)
        token = mx.argmax(logits[:, -1:, :], axis=-1)
        mx.eval(token)
        samples.append((time.perf_counter() - start) * 1_000)
    step = statistics.median(samples[2:])
    base = base or step
    print(f"{rows:>3} {step:>9.2f} {step / rows:>8.2f} {base / step * rows:>6.2f}x")
