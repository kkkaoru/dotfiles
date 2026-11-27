"""H9, measured: can the Neural Engine run a decode step faster than the GPU does?

The step is a chain of quantized matmuls whose weights are streamed once. This converts one
transformer block's four matmuls (qkv, o, gate_up, down at the shipped shapes) to CoreML, palettizes
the weights to 4 bits - CoreML's only four-bit form - and times the block on the Neural Engine, on the
GPU and on the CPU. A step is 16 blocks plus `lm_head`; the table multiplies the block by 16, so it
understates the ANE by leaving attention out.

Needs coremltools, which the translate server's own environment does not have:

    uv venv /tmp/ane-venv --python 3.12
    uv pip install --python /tmp/ane-venv/bin/python coremltools numpy torch
    /tmp/ane-venv/bin/python tools/plamo-swift/scripts/hypothesis-ane.py
"""

import statistics
import sys
import time

import coremltools as ct
import numpy as np
import torch

HIDDEN = 1536
FFN = 6144
QKV = HIDDEN + 2 * 128  # 16 query heads of 128 dims, one KV head of 128 twice
LAYERS = 16
BITS = 4
PATH = "/tmp/ane-block.mlpackage"


class Block(torch.nn.Module):
    def __init__(self) -> None:
        super().__init__()
        self.qkv = torch.nn.Linear(HIDDEN, QKV, bias=False)
        self.o = torch.nn.Linear(QKV, HIDDEN, bias=False)
        self.gate_up = torch.nn.Linear(HIDDEN, 2 * FFN, bias=False)
        self.down = torch.nn.Linear(2 * FFN, HIDDEN, bias=False)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        x = self.qkv(x)
        x = self.o(x)
        x = self.gate_up(x)
        return self.down(x)


def converted() -> str:
    model = Block().eval()
    weights = sum(parameter.numel() for parameter in model.parameters())
    print(f"one block: {weights / 1e6:.1f} M weights, {weights * BITS / 8 / 1e6:.1f} MB at {BITS} bits")
    mlmodel = ct.convert(
        torch.jit.trace(model, torch.zeros(1, HIDDEN)),
        inputs=[ct.TensorType(name="x", shape=(1, HIDDEN))],
        minimum_deployment_target=ct.target.macOS15,
        compute_precision=ct.precision.FLOAT16,
        convert_to="mlprogram",
    )
    mlmodel = ct.optimize.coreml.palettize_weights(
        mlmodel,
        ct.optimize.coreml.OptimizationConfig(
            global_config=ct.optimize.coreml.OpPalettizerConfig(nbits=BITS, mode="kmeans")
        ),
    )
    mlmodel.save(PATH)
    return PATH


def timed(path: str, compute: ct.ComputeUnit, rounds: int = 12) -> float:
    local = ct.models.MLModel(path, compute_units=compute)
    x = {"x": np.zeros((1, HIDDEN), dtype=np.float16)}
    local.predict(x)
    samples = []
    for _ in range(rounds):
        start = time.perf_counter()
        local.predict(x)
        samples.append((time.perf_counter() - start) * 1_000)
    return statistics.median(samples[2:])


path = converted()
print()
print(f"{'engine':<22} {'block ms':>9} {'step ms':>8} {'steps/s':>8}")
for compute, label in (
    (ct.ComputeUnit.CPU_AND_NE, "CPU + Neural Engine"),
    (ct.ComputeUnit.CPU_AND_GPU, "CPU + GPU"),
    (ct.ComputeUnit.CPU_ONLY, "CPU only"),
):
    block = timed(path, compute)
    print(f"{label:<22} {block:>9.3f} {block * LAYERS:>8.2f} {1000 / (block * LAYERS):>8.1f}")
