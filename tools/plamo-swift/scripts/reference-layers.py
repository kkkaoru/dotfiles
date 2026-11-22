#!/usr/bin/env python3
"""Stage-by-stage norms from mlx_lm, to locate where the Swift port diverges.

  reference-layers.py <model dir> <prompt>
"""
import sys

import mlx.core as mx
from mlx_lm import load
from mlx_lm.models.base import create_attention_mask

path, prompt = sys.argv[1], sys.argv[2]
model, tokenizer = load(path, tokenizer_config={"trust_remote_code": True}, trust_remote_code=True)
inner = model.model
ids = list(tokenizer.encode(prompt))


def show(stage, h):
    last = h[:, -1, :]
    print(
        f"{stage} norm {mx.sqrt((last * last).sum()).item():.4f} "
        + " ".join(f"{v:.4f}" for v in last[0, :3].tolist())
    )


h = inner.embed_tokens(mx.array([ids]))
show("embed", h)

decoder = inner.layers
mask = create_attention_mask(h, None)
for index, layer in enumerate(decoder.layers):
    residual = h
    h2 = layer.pre_mixer_norm(h)
    show(f"L{index}.pre_mixer", h2)
    sa = layer.mixer(h2, mask=mask, cache=None)
    show(f"L{index}.mixer", sa)
    h = residual + layer.post_mixer_norm(sa)
    residual = h
    mlp_out = layer.mlp(layer.pre_mlp_norm(h))
    show(f"L{index}.mlp", mlp_out)
    h = residual + layer.post_mlp_norm(mlp_out)
